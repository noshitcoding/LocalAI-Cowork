//! Core V1 Android wire adapter over actual tenant sessions, team runs and files.
//! Devices use independent credentials and one-shot leases, without owner adoption.
#[path = "actor_chat_configuration_routes.rs"]
mod actor_chat_configuration_routes;
#[path = "actor_context_routes.rs"]
mod actor_context_routes;
#[path = "android_admin_routes.rs"]
mod admin_routes;
#[path = "android_administration_routes.rs"]
mod administration;
#[path = "android_agent_routes.rs"]
mod agents;
#[path = "android_auth_security_routes.rs"]
mod auth_security;
#[path = "android_browser_auth.rs"]
mod browser_auth;
#[path = "android_desktop_routes.rs"]
mod desktop_devices;
#[path = "android_fleet_routes.rs"]
mod fleet_devices;
#[path = "android_oidc_routes.rs"]
mod oidc;
#[path = "android_passkey_routes.rs"]
mod passkeys;
#[path = "android_personal_routes.rs"]
mod personal;
#[path = "android_private_github_routes.rs"]
mod private_github_routes;
#[path = "android_private_mcp_routes.rs"]
mod private_mcp_routes;
#[path = "android_schedule_routes.rs"]
mod private_schedule_routes;
#[path = "android_private_terminal_routes.rs"]
mod private_terminal_routes;
#[path = "android_workflow_routes.rs"]
mod private_workflow_routes;
#[path = "android_project_version_routes.rs"]
mod project_versions;
#[path = "android_provider_routes.rs"]
mod provider_routes;
#[path = "android_push_routes.rs"]
mod push_devices;
#[path = "android_run_recovery_routes.rs"]
mod run_recovery;
#[path = "android_snapshot_routes.rs"]
mod snapshot_routes;
#[path = "android_sync_routes.rs"]
mod wire_sync;
use crate::s3_store::S3Store;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequestParts, Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use cowork_contracts::*;
use cowork_domain::{Error, Result, hash};
use cowork_identity::{Principal, SessionIssued};
use cowork_secrets::{
    CredentialScope, KeyId, KeyStore, SealedCredential, open_bearer, seal_bearer,
};
use cowork_storage::{
    AndroidAuthGrant, TEAM_BLOB_MAX_BYTES, TeamBlobState, TeamBlobUpload, now,
    team_service::{TeamService, identity_error},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    convert::Infallible,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;
use zeroize::Zeroizing;
const TTL: i64 = 8 * 60 * 60 * 1000;
#[derive(Clone)]
struct CompatState {
    service: TeamService,
    objects: Arc<S3Store>,
    keys: Arc<dyn KeyStore>,
    key_id: KeyId,
    devices: Arc<crate::android_device_services::AndroidDeviceServices>,
    limit: Arc<Mutex<(Instant, u32, u32)>>,
}
pub(crate) fn router(
    service: TeamService,
    objects: Arc<S3Store>,
    keys: Arc<dyn KeyStore>,
    key_id: KeyId,
    devices: Arc<crate::android_device_services::AndroidDeviceServices>,
) -> Router {
    let state = CompatState {
        service,
        objects,
        keys,
        key_id,
        devices,
        limit: Arc::new(Mutex::new((Instant::now(), 0, 0))),
    };
    Router::new()
        .merge(actor_context_routes::routes())
        .merge(actor_chat_configuration_routes::routes())
        .route("/version", get(version))
        .route("/capabilities", get(capabilities))
        .merge(passkeys::routes())
        .merge(push_devices::routes())
        .merge(admin_routes::routes())
        .merge(administration::routes())
        .merge(auth_security::routes())
        .merge(oidc::routes())
        .merge(agents::routes())
        .merge(desktop_devices::routes())
        .merge(fleet_devices::routes())
        .merge(personal::routes())
        .merge(wire_sync::routes())
        .merge(private_workflow_routes::routes())
        .merge(project_versions::routes())
        .merge(provider_routes::routes())
        .merge(snapshot_routes::routes())
        .merge(private_mcp_routes::routes())
        .merge(private_github_routes::routes())
        .merge(private_terminal_routes::routes())
        .merge(private_schedule_routes::routes())
        .merge(run_recovery::routes())
        .route("/auth/login", post(login))
        .route("/auth/native/authorize", post(native_authorize))
        .route("/auth/native/token", post(native_token))
        .route("/auth/refresh", post(refresh))
        .route("/auth/browser/refresh", post(browser_auth::refresh))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/teams", get(teams).post(admin_routes::create_team))
        .route("/projects", get(projects).post(create_project))
        .route(
            "/projects/{project}",
            get(project)
                .put(personal::update_project)
                .delete(personal::delete_project),
        )
        .route("/projects/{project}/threads", get(threads))
        .route("/threads", post(create_thread))
        .route(
            "/threads/{thread}",
            get(thread)
                .put(personal::update_thread)
                .delete(personal::delete_thread),
        )
        .route(
            "/threads/{thread}/messages",
            get(messages).post(message_run),
        )
        .route("/thread-runs", post(thread_run))
        .route("/sync/entities/{kind}", get(synced_entities))
        .route("/runs", get(runs).post(create_run))
        .route("/runs/{run}", get(run))
        .route("/runs/{run}/cancel", post(cancel))
        .route("/runs/{run}/events", get(events))
        .route("/runs/{run}/artifacts", get(artifacts))
        .route("/runs/{run}/artifacts/{artifact}", get(download))
        .route(
            "/runs/{run}/attachments",
            post(upload).layer(DefaultBodyLimit::max(TEAM_BLOB_MAX_BYTES as usize)),
        )
        .fallback(unsupported)
        .layer(DefaultBodyLimit::max(1_100_000))
        .layer(middleware::from_fn_with_state(state.clone(), protect))
        .with_state(state)
}
fn invalid() -> Error {
    Error::Validation("Invalid or unsupported Android compatibility request".into())
}
struct Failure(Error);
impl From<Error> for Failure {
    fn from(value: Error) -> Self {
        Self(value)
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let (status, code) = match self.0 {
            Error::Forbidden | Error::NotFound => (StatusCode::NOT_FOUND, "access_denied"),
            Error::Validation(message) if message == "knowledge_base_disabled" => {
                (StatusCode::FORBIDDEN, "knowledge_base_disabled")
            }
            Error::Validation(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            Error::Conflict | Error::UncertainEffect => (StatusCode::CONFLICT, "request_conflict"),
            _ => (StatusCode::SERVICE_UNAVAILABLE, "compatibility_unavailable"),
        };
        (status, Json(json!({"error":code,"message":code}))).into_response()
    }
}
type Answer = std::result::Result<Json<Value>, Failure>;
async fn unsupported() -> Response {
    (StatusCode::NOT_IMPLEMENTED,Json(json!({"error":"unsupported_compatibility_operation","message":"This V1 operation is not supported by the V2 Android core adapter"}))).into_response()
}
async fn protect(State(s): State<CompatState>, request: Request, next: Next) -> Response {
    let auth = request.uri().path().contains("/auth/");
    let browser_headers = request.headers().clone();
    let browser_path = request.uri().path().to_owned();
    match browser_auth::mode(&browser_headers) {
        Ok(true) if auth => {
            if browser_auth::origin(&s, &browser_headers).is_err() {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({"error":"browser_origin_required"})),
                )
                    .into_response();
            }
        }
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_browser_session_mode"})),
            )
                .into_response();
        }
        _ => {}
    }
    let allowed = if let Ok(mut limit) = s.limit.lock() {
        if limit.0.elapsed() >= Duration::from_secs(60) {
            *limit = (Instant::now(), 0, 0);
        }
        if limit.1 >= 600 || auth && limit.2 >= 20 {
            false
        } else {
            limit.1 += 1;
            if auth {
                limit.2 += 1;
            }
            true
        }
    } else {
        false
    };
    if !allowed {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"rate_limited"})),
        )
            .into_response();
    }
    let response = next.run(request).await;
    let mut response = browser_auth::boundary(&s, &browser_headers, &browser_path, response).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static header"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        "nosniff".parse().expect("static header"),
    );
    response
}
struct Session(Principal);
impl FromRequestParts<CompatState> for Session {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &CompatState,
    ) -> std::result::Result<Self, Response> {
        let mut headers = parts.headers.get_all(header::AUTHORIZATION).iter();
        let token = headers
            .next()
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if headers.next().is_some() || token.is_none() {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"authentication_required"})),
            )
                .into_response());
        }
        let identity = state.service.store().identity();
        let actor = identity
            .authenticate_session(token.unwrap_or(""))
            .await
            .map_err(|_| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error":"authentication_required"})),
                )
                    .into_response()
            })?;
        let path = parts
            .extensions
            .get::<axum::extract::OriginalUri>()
            .map(|v| v.0.path())
            .unwrap_or(parts.uri.path());
        identity
            .android_management_request(&actor, parts.method.as_str(), path)
            .await
            .map_err(|_| {
                (
                    StatusCode::FORBIDDEN,
                    Json(json!({"error":"management_mfa_required"})),
                )
                    .into_response()
            })?;
        Ok(Self(actor))
    }
}
fn iso(value: i64) -> Result<String> {
    chrono::DateTime::from_timestamp_millis(value)
        .map(|v| v.to_rfc3339())
        .ok_or(Error::Conflict)
}
fn device(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 200
        || !value.is_ascii()
        || value.chars().any(char::is_control)
    {
        Err(invalid())
    } else {
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthPayload {
    user_id: Uuid,
    session_id: Uuid,
    challenge: Option<String>,
    sealed: SealedCredential,
}
fn auth_scope(grant: &AndroidAuthGrant, payload: &AuthPayload) -> Result<CredentialScope> {
    Ok(CredentialScope {
        project_id: format!("android-auth:{}", grant.id),
        server_id: "android-compat/v1".into(),
        reference: hash(
            serde_json::to_vec(&(
                &grant.kind,
                &grant.device_id,
                grant.expires_at,
                payload.user_id,
                payload.session_id,
                &payload.challenge,
            ))
            .map_err(|_| invalid())?
            .as_slice(),
        ),
        revision: 1,
        endpoint: "localai-cowork://android/auth".into(),
    })
}
async fn make_grant(
    s: &CompatState,
    issued: &SessionIssued,
    device_id: &str,
    kind: &str,
    challenge: Option<String>,
    expires_at: i64,
) -> Result<String> {
    device(device_id)?;
    let cipher_key = s
        .keys
        .key_for_purpose(&s.key_id, cowork_secrets::KeyPurpose::DataEncryption)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let principal = s
        .service
        .store()
        .identity()
        .authenticate_session(&issued.token)
        .await
        .map_err(identity_error)?;
    let secret = Zeroizing::new(format!(
        "cw2a_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    ));
    let mut grant = AndroidAuthGrant {
        id: Uuid::new_v4().to_string(),
        kind: kind.into(),
        device_id: device_id.into(),
        expires_at,
        payload: String::new(),
    };
    let placeholder = CredentialScope {
        project_id: format!("android-auth:{}", grant.id),
        server_id: "android-compat/v1".into(),
        reference: "initial".into(),
        revision: 1,
        endpoint: "localai-cowork://android/auth".into(),
    };
    let sealed = seal_bearer(s.keys.as_ref(), cipher_key, &placeholder, &issued.token)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let mut payload = AuthPayload {
        user_id: principal.identity_id(),
        session_id: principal.session_id(),
        challenge,
        sealed,
    };
    payload.sealed = seal_bearer(
        s.keys.as_ref(),
        cipher_key,
        &auth_scope(&grant, &payload)?,
        &issued.token,
    )
    .map_err(|_| Error::NativeKeyUnavailable)?;
    grant.payload = serde_json::to_string(&payload).map_err(|_| invalid())?;
    s.service
        .store()
        .android_create_grant(&hash(secret.as_bytes()), &grant)
        .await?;
    Ok(secret.to_string())
}
async fn open_grant(s: &CompatState, grant: &AndroidAuthGrant) -> Result<(AuthPayload, Principal)> {
    let payload: AuthPayload = serde_json::from_str(&grant.payload).map_err(|_| invalid())?;
    let token = open_bearer(
        s.keys.as_ref(),
        &auth_scope(grant, &payload)?,
        &payload.sealed,
    )
    .map_err(|_| Error::NativeKeyUnavailable)?;
    let actor = s
        .service
        .store()
        .identity()
        .authenticate_session(&token)
        .await
        .map_err(identity_error)?;
    if actor.identity_id() != payload.user_id || actor.session_id() != payload.session_id {
        return Err(Error::Forbidden);
    }
    Ok((payload, actor))
}
async fn issued_tokens(s: &CompatState, issued: SessionIssued, device_id: &str) -> Result<Value> {
    s.service
        .store()
        .android_security_record_session(&issued, device_id)
        .await?;
    let token = Zeroizing::new(issued.token.clone());
    let actor = s
        .service
        .store()
        .identity()
        .authenticate_session(&token)
        .await
        .map_err(identity_error)?;
    let refresh_token =
        make_grant(s, &issued, device_id, "refresh", None, issued.expires_at).await?;
    if let Ok(device) = Uuid::parse_str(device_id) {
        s.service
            .store()
            .android_wire_bind_device(&actor, device)
            .await?;
    }
    Ok(
        json!({"schema_version":2,"access_token":*token,"access_expires_at":iso(issued.expires_at)?,"refresh_token":refresh_token,"refresh_expires_at":iso(issued.expires_at)?,"user_id":actor.identity_id(),"session_id":actor.session_id()}),
    )
}
type Credentials = AndroidPasswordCredentials;
async fn login(
    State(s): State<CompatState>,
    headers: HeaderMap,
    Json(value): Json<Credentials>,
) -> Answer {
    device(&value.device_id)?;
    let password = Zeroizing::new(value.password);
    let factor = value.second_factor.map(Zeroizing::new);
    Ok(Json(
        s.service
            .store()
            .android_security_login_tokens(
                s.keys.as_ref(),
                s.key_id,
                &value.email,
                &password,
                &value.device_id,
                factor.as_deref().map(String::as_str),
                TTL,
                auth_security::operation(&headers)?,
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Authorization {
    email: String,
    password: String,
    device_id: String,
    #[serde(default)]
    second_factor: Option<String>,
    code_challenge: String,
    code_challenge_method: String,
}
async fn native_authorize(
    State(s): State<CompatState>,
    headers: HeaderMap,
    Json(value): Json<Authorization>,
) -> Answer {
    if value.code_challenge_method != "S256"
        || value.code_challenge.len() != 43
        || !value
            .code_challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err(invalid().into());
    }
    device(&value.device_id)?;
    let password = Zeroizing::new(value.password);
    let factor = value.second_factor.map(Zeroizing::new);
    Ok(Json(
        s.service
            .store()
            .android_transport_authorize(
                s.keys.as_ref(),
                s.key_id,
                auth_security::operation(&headers)?,
                &value.email,
                &password,
                &value.device_id,
                factor.as_deref().map(String::as_str),
                &value.code_challenge,
                TTL,
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenExchange {
    code: String,
    code_verifier: String,
    device_id: String,
}
async fn native_token(
    State(s): State<CompatState>,
    headers: HeaderMap,
    Json(value): Json<TokenExchange>,
) -> Answer {
    device(&value.device_id)?;
    let secret = Zeroizing::new(value.code);
    let verifier = Zeroizing::new(value.code_verifier);
    Ok(Json(
        s.service
            .store()
            .android_transport_exchange(
                s.keys.as_ref(),
                s.key_id,
                auth_security::operation(&headers)?,
                "code",
                &secret,
                Some(&verifier),
                Some(&value.device_id),
                false,
                None,
                TTL,
            )
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Refresh {
    refresh_token: String,
}
async fn refresh(
    State(s): State<CompatState>,
    headers: HeaderMap,
    Json(value): Json<Refresh>,
) -> Answer {
    let secret = Zeroizing::new(value.refresh_token);
    Ok(Json(
        s.service
            .store()
            .android_transport_exchange(
                s.keys.as_ref(),
                s.key_id,
                auth_security::operation(&headers)?,
                "refresh",
                &secret,
                None,
                None,
                false,
                None,
                TTL,
            )
            .await?,
    ))
}

async fn logout(State(s): State<CompatState>, Session(a): Session) -> Answer {
    s.service
        .store()
        .identity()
        .logout(&a)
        .await
        .map_err(identity_error)?;
    Ok(Json(json!({"schema_version":2,"accepted":true})))
}
async fn me(State(s): State<CompatState>, Session(a): Session) -> Answer {
    let value = s
        .service
        .store()
        .identity()
        .self_identity(&a)
        .await
        .map_err(identity_error)?;
    Ok(Json(
        json!({"schema_version":2,"id":value.id,"email":s.service.store().android_security_email(&a).await?,"display_name":value.display_name,"active":value.active,"revision":value.revision}),
    ))
}
async fn version() -> Json<Value> {
    Json(
        json!({"api_version":"v1","schema_version":2,"minimum_compatible_schema_version":2,"build_version":env!("CARGO_PKG_VERSION"),"adapter":"v2-android-devices","limitations":["private_server_requires_linux_worker","configured_runner_tools_only","operator_configured_push_passkeys_fleet","explicit_device_pairing","no_implicit_v1_identity_adoption"],"attachment_limit_bytes":TEAM_BLOB_MAX_BYTES}),
    )
}
async fn capabilities(State(s): State<CompatState>, Session(a): Session) -> Answer {
    use cowork_storage::android_server_capabilities as caps;
    let mut scopes = std::collections::BTreeMap::<String, Vec<Value>>::new();
    if s.devices.private_server_available() {
        let mut input = json!({});
        if let Some(binding) = s.devices.private_runner_binding() {
            match s
                .service
                .store()
                .android_private_runner_current(&a, &binding)
                .await
            {
                Ok(()) => input["frozen_private_runner"] = binding,
                Err(Error::Forbidden | Error::NotFound) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let mcp = s
            .service
            .store()
            .android_private_mcp_configured(&a, !input["frozen_private_runner"].is_null())
            .await?;
        let private_model = s
            .service
            .store()
            .android_personal_providers(&a)
            .await?
            .iter()
            .any(|profile| {
                (profile["has_secret"] == true || profile["model_defaults"]["auth_mode"] == "none")
                    && profile["model_defaults"]["endpoint_binding"] != "per_device"
            });
        let mut tools = caps::private_tools(&input)?;
        if private_model && s.devices.private_workflow_available("workflow").await? {
            scopes
                .entry("workflow.pure".into())
                .or_default()
                .push(json!({"privacy":"private_local"}));
        }
        if private_model
            && !input["frozen_private_runner"].is_null()
            && s.devices.private_workflow_available("crew").await?
        {
            scopes
                .entry("crew.python".into())
                .or_default()
                .push(json!({"privacy":"private_local"}));
        }
        if input["frozen_private_runner"]["github"] == true
            && s.service
                .store()
                .android_private_github_configured(&a)
                .await?
        {
            tools.push("github".into());
        }
        for name in caps::capabilities_for_tools(&tools, caps::private_shell(&input), mcp) {
            if private_model || !name.starts_with("model.") {
                scopes
                    .entry(name)
                    .or_default()
                    .push(json!({"privacy":"private_local"}));
            }
        }
    }
    for team in s.service.teams(&a).await? {
        if s.service
            .provider(&a, team.id)
            .await?
            .is_some_and(|provider| provider.key_configured)
        {
            let tools = s.service.available_tools(&a, team.id).await?;
            for name in caps::capabilities_for_tools(
                &tools,
                false,
                tools.iter().any(|name| name.starts_with("mcp_")),
            ) {
                scopes
                    .entry(name)
                    .or_default()
                    .push(json!({"privacy":"team_managed","team_id":team.id}));
            }
        }
    }
    let descriptors: Vec<Value> = scopes.into_iter().map(|(name, scopes)| json!({"schema_version":2,"name":name,"version":"1","attributes":{"adapter":"v2_configured_tools","host_label_is_legacy_wire":true,"scopes":scopes,"admission":"fresh_actor_project_model_tool_authority"}})).collect();
    Ok(Json(
        json!({"schema_version":2,"server_linux":descriptors,"executors":s.service.store().android_devices(&a).await?}),
    ))
}
async fn team_wire(s: &CompatState, a: &Principal, t: &Team) -> Result<Value> {
    let (owner, created) = s
        .service
        .store()
        .android_creation_metadata(a, t.id, t.id, "organization_created")
        .await?;
    Ok(
        json!({"schema_version":2,"id":t.id,"revision":t.revision,"etag":format!("{}:{}",t.id,t.revision),"name":t.name,"owner_user_id":owner,"created_at":iso(created)?,"updated_at":iso(created)?,"deleted_at":if t.state=="closed"{Some(iso(created)?)}else{None}}),
    )
}
async fn teams(State(s): State<CompatState>, Session(a): Session) -> Answer {
    let mut values = Vec::new();
    for t in s.service.teams(&a).await? {
        values.push(team_wire(&s, &a, &t).await?);
    }
    Ok(Json(json!(values)))
}
async fn project_wire(s: &CompatState, a: &Principal, p: &TeamProject) -> Result<Value> {
    let (owner, _) = s
        .service
        .store()
        .android_creation_metadata(a, p.team_id, p.id, "content_resource_created")
        .await?;
    Ok(
        json!({"schema_version":2,"id":p.id,"revision":p.revision,"etag":format!("{}:{}",p.id,p.revision),"owner_user_id":owner,"team_id":p.team_id,"privacy":"team_managed","name":p.name,"description":"","preferred_executor_target":{"kind":"server_linux","pool_id":null},"current_version_id":null,"policy":{"adapter":"team_litellm","archived":p.archived},"created_at":iso(p.created_at)?,"updated_at":iso(p.created_at)?,"deleted_at":null}),
    )
}
async fn all_projects(s: &CompatState, a: &Principal) -> Result<Vec<TeamProject>> {
    let mut values = Vec::new();
    for team in s.service.teams(a).await? {
        for project in s.service.projects(a, team.id).await? {
            if values.len() >= 2000 {
                return Err(Error::BudgetExceeded);
            }
            values.push(project);
        }
    }
    Ok(values)
}
async fn projects(State(s): State<CompatState>, Session(a): Session) -> Answer {
    let mut values = s.service.store().android_personal_projects(&a).await?;
    for p in all_projects(&s, &a).await? {
        values.push(project_wire(&s, &a, &p).await?);
    }
    Ok(Json(json!(values)))
}
async fn project(State(s): State<CompatState>, Session(a): Session, Path(p): Path<Uuid>) -> Answer {
    if let Some(value) = s.service.store().android_personal_project(&a, p).await? {
        return Ok(Json(value));
    }
    let team = s.service.store().android_project_team(&a, p).await?;
    Ok(Json(
        project_wire(&s, &a, &s.service.project(&a, team, p).await?).await?,
    ))
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NewProject {
    name: String,
    description: String,
    privacy: String,
    team_id: Option<Uuid>,
    preferred_executor_target: Option<Value>,
    policy: Value,
    #[serde(default)]
    idempotency_key: Option<String>,
}
fn target(value: &Value) -> Result<()> {
    if value.get("kind").and_then(Value::as_str) != Some("server_linux")
        || value.get("pool_id").is_some_and(|v| !v.is_null())
    {
        return Err(invalid());
    }
    Ok(())
}
async fn create_project(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(v): Json<NewProject>,
) -> Answer {
    if v.privacy == "private_local" {
        if let Some(t) = &v.preferred_executor_target {
            personal::checked_target(t)?;
        }
        return Ok(Json(
            s.service
                .store()
                .android_personal_create_project(
                    &a,
                    serde_json::to_value(v).map_err(|_| invalid())?,
                )
                .await?,
        ));
    }
    if v.privacy != "team_managed"
        || !v.description.is_empty()
        || !v.policy.is_null() && !v.policy.as_object().is_some_and(|o| o.is_empty())
    {
        return Err(invalid().into());
    }
    if let Some(t) = v.preferred_executor_target {
        target(&t)?;
    }
    let team = v.team_id.ok_or_else(invalid)?;
    let key = v
        .idempotency_key
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let p = s
        .service
        .create_project(
            &a,
            team,
            CreateTeamProject {
                name: v.name,
                idempotency_key: key,
            },
        )
        .await?;
    Ok(Json(project_wire(&s, &a, &p).await?))
}
fn thread_wire(o: &TeamOrder) -> Value {
    json!({"schema_version":2,"id":o.id,"revision":o.revision,"etag":format!("{}:{}",o.id,o.revision),"project_id":o.project_id,"forked_from_thread_id":null,"forked_from_message_id":null,"title":o.title,"deleted_at":null})
}
async fn thread_wire_with_lineage(s: &CompatState, a: &Principal, o: &TeamOrder) -> Result<Value> {
    Ok(s.service
        .store()
        .android_project_thread_lineage(a, o.project_id, o.id)
        .await?
        .unwrap_or_else(|| thread_wire(o)))
}
async fn threads(State(s): State<CompatState>, Session(a): Session, Path(p): Path<Uuid>) -> Answer {
    if s.service
        .store()
        .android_personal_project(&a, p)
        .await?
        .is_some()
    {
        return Ok(Json(json!(
            s.service.store().android_personal_threads(&a, p).await?
        )));
    }
    let t = s.service.store().android_project_team(&a, p).await?;
    let mut values = Vec::new();
    for order in s.service.orders(&a, t, p).await? {
        values.push(thread_wire_with_lineage(&s, &a, &order).await?);
    }
    Ok(Json(json!(values)))
}
async fn thread(State(s): State<CompatState>, Session(a): Session, Path(o): Path<Uuid>) -> Answer {
    if let Some(v) = s.service.store().android_personal_thread(&a, o).await? {
        return Ok(Json(v));
    }
    Ok(Json(
        thread_wire_with_lineage(&s, &a, &s.service.store().android_order(&a, o).await?).await?,
    ))
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NewThread {
    project_id: Uuid,
    title: String,
    #[serde(default)]
    forked_from_thread_id: Option<Uuid>,
    #[serde(default)]
    forked_from_message_id: Option<Uuid>,
    #[serde(default)]
    idempotency_key: Option<String>,
}
async fn create_thread(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
    Json(mut v): Json<NewThread>,
) -> Answer {
    let first = headers
        .get("idempotency-key")
        .map(|v| v.to_str().map_err(|_| invalid()))
        .transpose()?;
    let second = headers
        .get("x-operation-id")
        .map(|v| v.to_str().map_err(|_| invalid()))
        .transpose()?;
    if first.is_some() && second.is_some() && first != second {
        return Err(Error::Conflict.into());
    }
    if let Some(key) = first.or(second) {
        if v.idempotency_key.as_deref().is_some_and(|body| body != key) {
            return Err(Error::Conflict.into());
        }
        v.idempotency_key = Some(key.to_owned());
    }
    if v.forked_from_thread_id.is_some() || v.forked_from_message_id.is_some() {
        return Ok(Json(
            s.service
                .store()
                .android_project_fork_thread(
                    &a,
                    AndroidForkThread {
                        project_id: v.project_id,
                        title: v.title,
                        forked_from_thread_id: v.forked_from_thread_id,
                        forked_from_message_id: v.forked_from_message_id,
                        idempotency_key: v.idempotency_key,
                    },
                )
                .await?,
        ));
    }
    if s.service
        .store()
        .android_personal_project(&a, v.project_id)
        .await?
        .is_some()
    {
        let key = v.idempotency_key.clone();
        return Ok(Json(
            s.service
                .store()
                .android_personal_create_thread(
                    &a,
                    serde_json::to_value(v).map_err(|_| invalid())?,
                    key,
                )
                .await?,
        ));
    }
    let t = s
        .service
        .store()
        .android_project_team(&a, v.project_id)
        .await?;
    let value = s
        .service
        .create_order(
            &a,
            t,
            v.project_id,
            CreateTeamOrder {
                title: v.title,
                prompt: String::new(),
                idempotency_key: v
                    .idempotency_key
                    .unwrap_or_else(|| Uuid::new_v4().to_string()),
            },
        )
        .await?;
    Ok(Json(thread_wire(&value)))
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunInput {
    thread_id: Uuid,
    project_id: Uuid,
    project_revision: i64,
    project_privacy: String,
    #[serde(default)]
    task: Option<Value>,
    executor_target: Value,
    required_capabilities: Vec<String>,
    input: Value,
    #[serde(default)]
    model_profile_id: Option<Uuid>,
    #[serde(default)]
    snapshot_id: Option<Uuid>,
    idempotency_key: String,
}
fn prompt(input: &Value) -> Result<String> {
    let p = if let Some(p) = input.as_str() {
        p
    } else {
        input
            .get("prompt")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
    };
    if p.trim().is_empty() || p.len() > 128 * 1024 {
        return Err(invalid());
    }
    Ok(p.to_owned())
}
fn ids(input: &Value, name: &str) -> Result<Vec<Uuid>> {
    match input.get(name) {
        None => Ok(Vec::new()),
        Some(v) => {
            let values = v.as_array().ok_or_else(invalid)?;
            if values.len() > 100 {
                return Err(invalid());
            }
            values
                .iter()
                .map(|v| {
                    v.as_str()
                        .ok_or_else(invalid)
                        .and_then(|s| Uuid::parse_str(s).map_err(|_| invalid()))
                })
                .collect()
        }
    }
}
fn message_text(value: &Value) -> Result<String> {
    let text = if let Some(text) = value.as_str() {
        text
    } else {
        let object = value.as_object().ok_or_else(invalid)?;
        if object.len() != 1 {
            return Err(invalid());
        }
        object
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
    };
    prompt(&json!({"prompt":text}))
}
async fn checked_run(s: &CompatState, a: &Principal, v: &RunInput) -> Result<TeamRun> {
    if v.project_privacy != "team_managed" || v.snapshot_id.is_some() {
        return Err(invalid());
    }
    target(&v.executor_target)?;
    let order = s.service.store().android_order(a, v.thread_id).await?;
    if order.project_id != v.project_id {
        return Err(Error::Forbidden);
    }
    if v.model_profile_id.is_some_and(|id| id != order.team_id) {
        return Err(invalid());
    }
    let available = s.service.available_tools(a, order.team_id).await?;
    let selected_mcp = ids(&v.input, "mcp_metadata_ids")?;
    let mut tools = std::collections::BTreeSet::new();
    let mut seen_mcp = std::collections::BTreeSet::new();
    for server in selected_mcp {
        if !seen_mcp.insert(server) {
            return Err(invalid());
        }
        let configured = s.service.mcp_tools(a, order.team_id, server).await?;
        let selected: Vec<String> = configured
            .into_iter()
            .map(|tool| tool.name)
            .filter(|name| available.contains(name))
            .collect();
        if selected.is_empty() {
            return Err(Error::Forbidden);
        }
        tools.extend(selected);
    }
    if let Some(selected) = v.input["task_config"].get("allowed_tools") {
        let selected = selected
            .as_array()
            .filter(|values| values.len() <= 32)
            .ok_or_else(invalid)?;
        let mcp = tools.clone();
        tools.clear();
        for name in selected {
            let name = name.as_str().ok_or_else(invalid)?;
            if !available.iter().any(|tool| tool == name)
                || (name.starts_with("mcp_") && !mcp.contains(name))
                || !tools.insert(name.to_owned())
            {
                return Err(Error::Forbidden);
            }
        }
    } else {
        for capability in &v.required_capabilities {
            let tool = match capability.as_str() {
                "files" | "tool.program.execute" => Some("execute_program"),
                "browser.headless" => Some("browser"),
                "office.ooxml" | "office.libreoffice" => Some("office"),
                "tool.github" => Some("github"),
                "model.chat" | "model.external" | "tool.mcp.invoke" => None,
                _ => return Err(invalid()),
            };
            if let Some(tool) = tool {
                if !available.iter().any(|name| name == tool) {
                    return Err(Error::Forbidden);
                }
                tools.insert(tool.into());
            }
        }
    }
    let tools: Vec<String> = tools.into_iter().collect();
    cowork_storage::android_server_capabilities::check_required(
        &v.required_capabilities,
        &cowork_storage::android_server_capabilities::capabilities_for_tools(
            &tools,
            false,
            tools.iter().any(|name| name.starts_with("mcp_")),
        ),
    )?;
    if let Some(o) = v.input.as_object()
        && o.keys().any(|key| {
            !matches!(
                key.as_str(),
                "prompt"
                    | "skill_ids"
                    | "memory_ids"
                    | "mcp_metadata_ids"
                    | "task_instructions"
                    | "task_config"
                    | "task_runner"
            )
        })
    {
        return Err(invalid());
    }
    s.service
        .create_run_at_project_revision(
            a,
            order.team_id,
            CreateTeamRun {
                order_id: order.id,
                prompt: if v.task.is_some() {
                    format!(
                        "Task instructions:\n{}\n\nUser request:\n{}",
                        v.input["task_instructions"].as_str().ok_or_else(invalid)?,
                        prompt(&v.input)?
                    )
                } else {
                    prompt(&v.input)?
                },
                idempotency_key: v.idempotency_key.clone(),
                plan: TeamRunPlan {
                    output: None,
                    tools,
                    skills: ids(&v.input, "skill_ids")?,
                    knowledge: ids(&v.input, "memory_ids")?,
                    ..Default::default()
                },
            },
            v.project_revision,
        )
        .await
}
async fn checked_wire_run(s: &CompatState, a: &Principal, v: &RunInput) -> Result<Value> {
    personal::checked_target(&v.executor_target)?;
    let raw_digest = hash(serde_json::to_vec(v).map_err(|_| invalid())?.as_slice());
    if matches!(
        v.executor_target["kind"].as_str(),
        Some("personal_device" | "managed_windows_pool")
    ) && let Some(receipt) = s
        .service
        .store()
        .android_device_original_receipt(a, v.project_id, &v.idempotency_key, &raw_digest)
        .await?
    {
        return Ok(receipt);
    }
    let prior = if v.project_privacy == "private_local" {
        s.service
            .store()
            .android_personal_run_intent(a, v.project_id, &v.idempotency_key, &raw_digest, None)
            .await?
    } else {
        None
    };
    let mut frozen = match &prior {
        Some(value) => serde_json::from_value(value.clone()).map_err(|_| invalid())?,
        None => v.clone(),
    };
    if prior.is_none() {
        if let Some(input) = frozen.input.as_object_mut() {
            for key in [
                "frozen_runtime_context",
                "crew_definition",
                "crew_entity_id",
                "crew_revision",
                "task_instructions",
                "task_config",
                "task_runner",
                "provider_binding",
                "frozen_project_files",
                "private_workflow",
                "device_crew",
            ] {
                input.remove(key);
            }
        }
        personal::freeze_task(s, a, &mut frozen).await?;
        if frozen.project_privacy == "private_local" {
            if frozen.input.is_string() {
                frozen.input = json!({"prompt":frozen.input});
            }
            personal::freeze_project_files(s, a, &mut frozen).await?;
            if frozen.executor_target["kind"] == "server_linux" {
                if !s.devices.private_server_available() {
                    return Err(Error::Validation(
                        "Private server execution requires a Linux worker".into(),
                    ));
                }
                frozen
                    .input
                    .as_object_mut()
                    .ok_or_else(invalid)?
                    .remove("private_runner_authority");
                if let Some(binding) = frozen
                    .input
                    .get("frozen_private_runner")
                    .filter(|value| !value.is_null())
                    .cloned()
                {
                    match s
                        .service
                        .store()
                        .android_private_runner_admission(a, frozen.project_id, &binding)
                        .await
                    {
                        Ok(stamp) => {
                            frozen.input["private_runner_authority"] = stamp;
                        }
                        Err(Error::Forbidden | Error::NotFound) => {
                            frozen
                                .input
                                .as_object_mut()
                                .ok_or_else(invalid)?
                                .remove("frozen_private_runner");
                        }
                        Err(error) => return Err(error),
                    }
                }
                let profile = s
                    .service
                    .store()
                    .android_personal_provider(a, frozen.model_profile_id.ok_or_else(invalid)?)
                    .await?;
                if let Some(expected) = frozen.input.get("expected_provider_revision")
                    && expected.as_i64().filter(|v| *v > 0) != profile["revision"].as_i64()
                {
                    return Err(Error::Conflict);
                }
                let binding = cowork_storage::android_personal_provider::binding(&profile)?;
                if frozen.input.is_string() {
                    frozen.input = json!({"prompt":frozen.input});
                }
                frozen
                    .input
                    .as_object_mut()
                    .ok_or_else(invalid)?
                    .insert("provider_binding".into(), binding);
            }
            let mut metadata = serde_json::Map::new();
            for (key, kind) in [
                ("skill_ids", "skill"),
                ("memory_ids", "memory"),
                ("mcp_metadata_ids", "mcp_metadata"),
            ] {
                let mut selected = personal::metadata_ids(&frozen.input, key)?;
                if ["skill_ids", "memory_ids"].contains(&key)
                    && !frozen.input["private_workflow"].is_null()
                {
                    frozen.input["private_workflow"]["context_selection"][if key == "skill_ids" {
                        "parent_skill_ids"
                    } else {
                        "parent_memory_ids"
                    }] = json!(selected);
                }
                if key != "mcp_metadata_ids"
                    && let Some(ids) =
                        frozen.input["private_workflow"]["context_selection"][key].as_array()
                {
                    for id in ids {
                        let id = Uuid::parse_str(id.as_str().ok_or_else(invalid)?)
                            .map_err(|_| invalid())?;
                        if !selected.contains(&id) {
                            selected.push(id);
                        }
                    }
                    if selected.len() > 100 {
                        return Err(Error::BudgetExceeded);
                    }
                    frozen.input[key] = json!(selected);
                }
                if !selected.is_empty() {
                    metadata.insert(
                        kind.into(),
                        json!(
                            s.service
                                .store()
                                .android_wire_runtime_metadata(
                                    a,
                                    frozen.project_id,
                                    kind,
                                    &selected
                                )
                                .await?
                        ),
                    );
                }
            }
            if !metadata.is_empty() {
                let input = frozen.input.as_object_mut().ok_or_else(invalid)?;
                input.insert("frozen_runtime_context".into(), Value::Object(metadata));
            }
            s.service
                .store()
                .freeze_actor_run_defaults(a, &mut frozen.input)
                .await?;
            let value = serde_json::to_value(&frozen).map_err(|_| invalid())?;
            let original = s
                .service
                .store()
                .android_personal_run_intent(
                    a,
                    v.project_id,
                    &v.idempotency_key,
                    &raw_digest,
                    Some(&value),
                )
                .await?
                .ok_or(Error::Conflict)?;
            frozen = serde_json::from_value(original).map_err(|_| invalid())?;
        }
    }
    let v = &frozen;
    if v.project_privacy == "private_local" {
        let project = s
            .service
            .store()
            .android_personal_project(a, v.project_id)
            .await?
            .ok_or(Error::NotFound)?;
        if v.executor_target["kind"] != "server_linux"
            && project["revision"].as_i64() != Some(v.project_revision)
        {
            return Err(Error::Conflict);
        }
        let thread = s
            .service
            .store()
            .android_personal_thread(a, v.thread_id)
            .await?
            .ok_or(Error::NotFound)?;
        if thread["project_id"] != json!(v.project_id) {
            return Err(Error::Forbidden);
        }
        prompt(&v.input)?;
        if v.executor_target["kind"] == "server_linux" {
            return s
                .service
                .store()
                .android_private_server_create(a, &serde_json::to_value(v).map_err(|_| invalid())?)
                .await;
        }
        if v.executor_target["kind"] != "personal_device" {
            return Err(invalid());
        }
        return s
            .service
            .store()
            .android_device_create_run_with_original(
                a,
                cowork_storage::AndroidDeviceRunRequest {
                    team: None,
                    project: v.project_id,
                    order: v.thread_id,
                    project_revision: v.project_revision,
                    target: v.executor_target.clone(),
                    capabilities: v.required_capabilities.clone(),
                    input: v.input.clone(),
                    model_profile: v.model_profile_id,
                    snapshot: v.snapshot_id,
                    key: v.idempotency_key.clone(),
                    task: v.task.clone(),
                },
                Vec::new(),
                &raw_digest,
            )
            .await;
    }
    if v.executor_target["kind"].as_str() == Some("server_linux") {
        let r = checked_run(s, a, v).await?;
        if let Some(task) = v.task.as_ref() {
            s.service
                .store()
                .android_team_run_task(a, r.id, Some(task), Some(&v.input))
                .await?;
        }
        return run_wire(s, a, &r).await;
    }
    if v.project_privacy != "team_managed" {
        return Err(invalid());
    }
    prompt(&v.input)?;
    let order = s.service.store().android_order(a, v.thread_id).await?;
    if order.project_id != v.project_id {
        return Err(Error::Forbidden);
    }
    if v.model_profile_id.is_some_and(|id| id != order.team_id) {
        return Err(invalid());
    }
    // Profiles identify compatibility selection, never disclose the team provider secret.
    let mut frozen_files = Vec::new();
    let selected = ids(&v.input, "project_file_ids")?;
    if selected.len() > 2000 || (!selected.is_empty() && v.snapshot_id.is_some()) {
        return Err(invalid());
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut total = 0usize;
    for id in selected {
        if !seen.insert(id) {
            return Err(invalid());
        }
        let version = match v
            .input
            .get("project_file_versions")
            .and_then(|versions| versions.get(id.to_string()))
        {
            Some(value) => Some(
                value
                    .as_i64()
                    .filter(|value| *value > 0)
                    .ok_or_else(invalid)?,
            ),
            None => None,
        };
        let read = s
            .service
            .store()
            .team_blob_read(a, order.team_id, v.project_id, id, version)
            .await?;
        let bytes = s.objects.get(read.version()).await?;
        total = total
            .checked_add(bytes.len())
            .ok_or(Error::BudgetExceeded)?;
        if bytes.len() > 32 * 1024 * 1024 || total > 256 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        if bytes.len() as u64 != read.version().size || hash(&bytes) != read.version().sha256 {
            return Err(Error::Conflict);
        }
        s.service.store().team_blob_read_finish(a, &read).await?;
        frozen_files.push((read.metadata().name.clone(), bytes));
    }
    s.service
        .store()
        .android_device_create_run_with_original(
            a,
            cowork_storage::AndroidDeviceRunRequest {
                team: Some(order.team_id),
                project: v.project_id,
                order: v.thread_id,
                project_revision: v.project_revision,
                target: v.executor_target.clone(),
                capabilities: v.required_capabilities.clone(),
                input: v.input.clone(),
                model_profile: v.model_profile_id,
                snapshot: v.snapshot_id,
                key: v.idempotency_key.clone(),
                task: v.task.clone(),
            },
            frozen_files,
            &raw_digest,
        )
        .await
}
fn wire_message(run: &Value, role: &str, content: Value, author: Option<Uuid>) -> Result<Value> {
    let id = run["spec"]["id"]
        .as_str()
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(invalid)?;
    Ok(
        json!({"schema_version":2,"id":if role=="user"{id}else{Uuid::from_u128(id.as_u128()^1)},"revision":1,"etag":hash(content.to_string().as_bytes()),"thread_id":run["spec"]["thread_id"],"author_user_id":author,"role":role,"content":content,"run_id":id,"created_at":if role=="user"{&run["spec"]["created_at"]}else{&run["updated_at"]},"updated_at":run["updated_at"],"deleted_at":null}),
    )
}
fn state(run: &TeamRun) -> &'static str {
    match run.status {
        TeamRunStatus::Queued => "queued",
        TeamRunStatus::Running => "running",
        TeamRunStatus::Completed => "completed",
        TeamRunStatus::Failed => "failed",
        TeamRunStatus::Unknown => "interrupted",
        TeamRunStatus::Cancelled => "canceled",
    }
}
async fn run_wire(s: &CompatState, a: &Principal, value: &TeamRun) -> Result<Value> {
    let (run, request, creator) = s.service.store().android_run_request(a, value.id).await?;
    let project_revision = s
        .service
        .store()
        .android_run_project_revision(a, run.id)
        .await?;
    let frozen = s
        .service
        .store()
        .android_team_run_task(a, run.id, None, None)
        .await?;
    let task = frozen.as_ref().map(|v| v.0.clone());
    let input = frozen
        .map(|v| v.1)
        .unwrap_or_else(|| json!({"prompt":request.prompt}));
    Ok(
        json!({"spec":{"schema_version":2,"id":run.id,"thread_id":run.order_id,"project_id":run.project_id,"project":{"id":run.project_id,"revision":project_revision},"project_privacy":"team_managed","task":task,"creator_user_id":creator,"executor_target":{"kind":"server_linux","pool_id":null},"required_capabilities":[],"input":input,"model_profile_id":run.team_id,"snapshot_id":null,"idempotency_key":request.idempotency_key,"created_at":iso(run.created_at)?},"state":state(&run),"revision":run.updated_at.max(1),"etag":hash(serde_json::to_vec(&run).map_err(|_|invalid())?.as_slice()),"assigned_executor_id":null,"lease_expires_at":null,"started_at":null,"finished_at":if matches!(run.status,TeamRunStatus::Completed|TeamRunStatus::Failed|TeamRunStatus::Cancelled){Some(iso(run.updated_at)?)}else{None},"result":run.response,"error":run.error.map(|message|json!({"code":"team_run_failed","message":message,"retryable":false})),"updated_at":iso(run.updated_at)?}),
    )
}
async fn create_run(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(v): Json<RunInput>,
) -> Answer {
    if v.project_privacy == "private_local" {
        let digest = hash(serde_json::to_vec(&v).map_err(|_| invalid())?.as_slice());
        let key = format!("run:{}", v.idempotency_key);
        if let Some(value) = s
            .service
            .store()
            .android_personal_operation(&a, v.project_id, &key, &digest, None)
            .await?
        {
            return Ok(Json(value));
        }
        let value = checked_wire_run(&s, &a, &v).await?;
        return Ok(Json(
            s.service
                .store()
                .android_personal_operation(&a, v.project_id, &key, &digest, Some(&value))
                .await?
                .ok_or(Error::Conflict)?,
        ));
    }
    let order = s.service.store().android_order(&a, v.thread_id).await?;
    let digest = hash(serde_json::to_vec(&v).map_err(|_| invalid())?.as_slice());
    let key = format!("run:{}", v.idempotency_key);
    if let Some(receipt) = s
        .service
        .store()
        .android_wire_operation(&a, order.team_id, order.project_id, &key, &digest)
        .await?
    {
        return Ok(Json(receipt));
    }
    let value = checked_wire_run(&s, &a, &v).await?;
    Ok(Json(
        s.service
            .store()
            .android_wire_finish(&a, order.team_id, order.project_id, &key, &digest, &value)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    #[serde(default)]
    limit: Option<usize>,
}
async fn runs(
    State(s): State<CompatState>,
    Session(a): Session,
    Query(q): Query<ListQuery>,
) -> Answer {
    let limit = q.limit.unwrap_or(100).clamp(1, 200);
    let mut all = Vec::new();
    for p in all_projects(&s, &a).await? {
        all.extend(s.service.runs(&a, p.team_id, p.id).await?);
    }
    all.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    let mut values = Vec::new();
    for r in all.into_iter().take(limit) {
        values.push(run_wire(&s, &a, &r).await?);
    }
    for p in all_projects(&s, &a).await? {
        values.extend(
            s.service
                .store()
                .android_device_runs(&a, p.team_id, p.id)
                .await?,
        );
    }
    for project in s.service.store().android_personal_projects(&a).await? {
        let id = project["id"]
            .as_str()
            .and_then(|v| Uuid::parse_str(v).ok())
            .ok_or_else(invalid)?;
        values.extend(s.service.store().android_personal_runs(&a, id).await?);
    }
    values.sort_by_key(|v| {
        std::cmp::Reverse(
            v["spec"]["created_at"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        )
    });
    values.truncate(limit);
    Ok(Json(json!({"items":values,"next_cursor":null})))
}
async fn run(State(s): State<CompatState>, Session(a): Session, Path(id): Path<Uuid>) -> Answer {
    if let Some(v) = s.service.store().android_private_server_run(&a, id).await? {
        return Ok(Json(v));
    }
    if let Some(v) = s.service.store().android_device_run(&a, id).await? {
        return Ok(Json(v));
    }
    let (r, _, _) = s.service.store().android_run_request(&a, id).await?;
    Ok(Json(run_wire(&s, &a, &r).await?))
}
async fn cancel(State(s): State<CompatState>, Session(a): Session, Path(id): Path<Uuid>) -> Answer {
    if s.service
        .store()
        .android_private_server_run(&a, id)
        .await?
        .is_some()
    {
        return Ok(Json(
            s.service
                .store()
                .android_private_server_cancel(&a, id)
                .await?,
        ));
    }
    if s.service
        .store()
        .android_device_run(&a, id)
        .await?
        .is_some()
    {
        return Ok(Json(s.service.store().android_device_cancel(&a, id).await?));
    }
    let (r, _, _) = s.service.store().android_run_request(&a, id).await?;
    let r = s.service.cancel_run(&a, r.team_id, id).await?;
    Ok(Json(run_wire(&s, &a, &r).await?))
}
fn message_wire(run: &TeamRun, role: &str, content: Value, author: Option<Uuid>) -> Result<Value> {
    let id = if role == "user" {
        run.id
    } else {
        Uuid::from_u128(run.id.as_u128() ^ 1)
    };
    Ok(
        json!({"schema_version":2,"id":id,"revision":1,"etag":hash(content.to_string().as_bytes()),"thread_id":run.order_id,"author_user_id":author,"role":role,"content":content,"run_id":run.id,"created_at":iso(if role=="user"{run.created_at}else{run.updated_at})?,"updated_at":iso(if role=="user"{run.created_at}else{run.updated_at})?,"deleted_at":null}),
    )
}
async fn messages(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Answer {
    if let Some(thread) = s.service.store().android_personal_thread(&a, id).await? {
        let project = thread["project_id"]
            .as_str()
            .and_then(|v| Uuid::parse_str(v).ok())
            .ok_or_else(invalid)?;
        let mut values = s.service.store().android_personal_messages(&a, id).await?;
        values.extend(
            s.service
                .store()
                .android_project_copied_messages(&a, project, id)
                .await?,
        );
        for run in s.service.store().android_personal_runs(&a, project).await? {
            if run["spec"]["thread_id"] != json!(id) {
                continue;
            }
            let user = wire_message(
                &run,
                "user",
                json!(prompt(&run["spec"]["input"])?),
                Some(a.identity_id()),
            )?;
            values.retain(|v| v["id"] != user["id"]);
            values.push(user);
            if let Some(content) = run["result"]["content"].as_str() {
                let answer = wire_message(&run, "assistant", json!(content), None)?;
                values.retain(|v| v["id"] != answer["id"]);
                values.push(answer);
            }
        }
        values.sort_by_key(|v| v["created_at"].as_str().unwrap_or_default().to_owned());
        let limit = q.limit.unwrap_or(100).clamp(1, 1000);
        if values.len() > limit {
            values.drain(..values.len() - limit);
        }
        return Ok(Json(json!(values)));
    }
    let o = s.service.store().android_order(&a, id).await?;
    let mut runs = s.service.runs(&a, o.team_id, o.project_id).await?;
    runs.retain(|r| r.order_id == id);
    runs.sort_by_key(|r| r.created_at);
    let mut values = s
        .service
        .store()
        .android_project_copied_messages(&a, o.project_id, id)
        .await?;
    for r in runs {
        let (_, request, creator) = s.service.store().android_run_request(&a, r.id).await?;
        let frozen = s
            .service
            .store()
            .android_team_run_task(&a, r.id, None, None)
            .await?;
        let user_content = frozen
            .map(|(_, input)| prompt(&input))
            .transpose()?
            .unwrap_or(request.prompt);
        values.push(message_wire(
            &r,
            "user",
            json!(user_content),
            Some(creator),
        )?);
        if let Some(answer) = &r.response {
            values.push(message_wire(&r, "assistant", json!(answer), None)?);
        }
    }
    for r in s
        .service
        .store()
        .android_device_runs(&a, o.team_id, o.project_id)
        .await?
    {
        if r["spec"]["thread_id"].as_str() != Some(id.to_string().as_str()) {
            continue;
        }
        values.push(wire_message(
            &r,
            "user",
            json!(prompt(&r["spec"]["input"])?),
            r["spec"]["creator_user_id"]
                .as_str()
                .and_then(|v| Uuid::parse_str(v).ok()),
        )?);
        if let Some(text) = r["result"]["content"].as_str() {
            values.push(wire_message(&r, "assistant", json!(text), None)?);
        }
    }
    values.sort_by_key(|v| v["created_at"].as_str().unwrap_or_default().to_owned());
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    if values.len() > limit {
        values.drain(..values.len() - limit);
    }
    Ok(Json(json!(values)))
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MessageRun {
    content: Value,
    run: RunInput,
}
async fn message_run(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(v): Json<MessageRun>,
) -> Answer {
    if v.run.thread_id != id || message_text(&v.content)? != prompt(&v.run.input)? {
        return Err(invalid().into());
    }
    if v.run.project_privacy == "private_local" {
        let digest = hash(serde_json::to_vec(&v).map_err(|_| invalid())?.as_slice());
        let key = format!("message:{}", v.run.idempotency_key);
        if let Some(receipt) = s
            .service
            .store()
            .android_personal_operation(&a, v.run.project_id, &key, &digest, None)
            .await?
        {
            return Ok(Json(receipt));
        }
        let run = checked_wire_run(&s, &a, &v.run).await?;
        let value = json!({"schema_version":2,"message":wire_message(&run,"user",v.content,Some(a.identity_id()))?,"run":run});
        return Ok(Json(
            s.service
                .store()
                .android_personal_operation(&a, v.run.project_id, &key, &digest, Some(&value))
                .await?
                .ok_or(Error::Conflict)?,
        ));
    }
    let o = s.service.store().android_order(&a, id).await?;
    let digest = hash(serde_json::to_vec(&v).map_err(|_| invalid())?.as_slice());
    let key = format!("message:{}", v.run.idempotency_key);
    if let Some(receipt) = s
        .service
        .store()
        .android_wire_operation(&a, o.team_id, o.project_id, &key, &digest)
        .await?
    {
        return Ok(Json(receipt));
    }
    let r = checked_wire_run(&s, &a, &v.run).await?;
    let value = json!({"schema_version":2,"message":wire_message(&r,"user",v.content,Some(a.identity_id()))?,"run":r});
    Ok(Json(
        s.service
            .store()
            .android_wire_finish(&a, o.team_id, o.project_id, &key, &digest, &value)
            .await?,
    ))
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NewThreadRun {
    title: String,
    content: Value,
    run: Value,
}
async fn thread_run(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(v): Json<NewThreadRun>,
) -> Answer {
    let mut input = v.run.clone();
    let object = input.as_object_mut().ok_or_else(invalid)?;
    if object.contains_key("thread_id") {
        return Err(invalid().into());
    }
    let project = object
        .get("project_id")
        .and_then(Value::as_str)
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(invalid)?;
    let key = object
        .get("idempotency_key")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?
        .to_owned();
    if object.get("project_privacy").and_then(Value::as_str) == Some("private_local") {
        let digest = hash(serde_json::to_vec(&v).map_err(|_| invalid())?.as_slice());
        let op = format!("thread-run:{key}");
        if let Some(receipt) = s
            .service
            .store()
            .android_personal_operation(&a, project, &op, &digest, None)
            .await?
        {
            return Ok(Json(receipt));
        }
        if message_text(&v.content)? != prompt(object.get("input").ok_or_else(invalid)?)? {
            return Err(invalid().into());
        }
        let thread=s.service.store().android_personal_create_thread(&a,json!({"project_id":project,"title":v.title,"forked_from_thread_id":null,"forked_from_message_id":null}),Some(format!("android:{}",hash(key.as_bytes())))).await?;
        object.insert("thread_id".into(), thread["id"].clone());
        let request: RunInput = serde_json::from_value(input).map_err(|_| invalid())?;
        let run = checked_wire_run(&s, &a, &request).await?;
        let value = json!({"schema_version":2,"thread":thread,"message":wire_message(&run,"user",v.content,Some(a.identity_id()))?,"run":run});
        return Ok(Json(
            s.service
                .store()
                .android_personal_operation(&a, project, &op, &digest, Some(&value))
                .await?
                .ok_or(Error::Conflict)?,
        ));
    }
    let team = s.service.store().android_project_team(&a, project).await?;
    let digest = hash(serde_json::to_vec(&v).map_err(|_| invalid())?.as_slice());
    let op = format!("thread-run:{key}");
    if let Some(receipt) = s
        .service
        .store()
        .android_wire_operation(&a, team, project, &op, &digest)
        .await?
    {
        return Ok(Json(receipt));
    }
    let p = prompt(object.get("input").ok_or_else(invalid)?)?;
    if message_text(&v.content)? != p {
        return Err(invalid().into());
    }
    let order = s
        .service
        .create_order(
            &a,
            team,
            project,
            CreateTeamOrder {
                title: v.title,
                prompt: String::new(),
                idempotency_key: format!("android:{}", hash(key.as_bytes())),
            },
        )
        .await?;
    object.insert("thread_id".into(), json!(order.id));
    let request: RunInput = serde_json::from_value(input).map_err(|_| invalid())?;
    let r = checked_wire_run(&s, &a, &request).await?;
    let value = json!({"schema_version":2,"thread":thread_wire(&order),"message":wire_message(&r,"user",v.content,Some(a.identity_id()))?,"run":r});
    Ok(Json(
        s.service
            .store()
            .android_wire_finish(&a, team, project, &op, &digest, &value)
            .await?,
    ))
}
async fn providers(State(s): State<CompatState>, Session(a): Session) -> Answer {
    Ok(Json(json!(provider_inventory(&s, &a).await?)))
}
async fn provider_inventory(s: &CompatState, a: &Principal) -> Result<Vec<Value>> {
    let mut values = match s.service.store().android_personal_providers(a).await {
        Ok(v) => v,
        Err(Error::Forbidden) => Vec::new(),
        Err(e) => return Err(e),
    };
    for team in s.service.teams(a).await? {
        if let Some(p) = s.service.provider(a, team.id).await? {
            let (owner, created) = s
                .service
                .store()
                .android_creation_metadata(a, team.id, team.id, "organization_created")
                .await?;
            values.push(json!({"schema_version":2,"id":team.id,"revision":p.revision,"etag":format!("{}:{}",team.id,p.revision),"owner_user_id":owner,"team_id":team.id,"name":format!("{} · {}",team.name,p.model),"provider_kind":"openai_compatible","model_defaults":{"model":p.model,"executor_kinds":["server_linux"]},"has_secret":p.key_configured,"created_at":iso(created)?,"updated_at":iso(created)?,"deleted_at":null}));
        }
    }
    Ok(values)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntityQuery {
    #[serde(default)]
    after: Option<Uuid>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default, alias = "since", alias = "after_cursor")]
    cursor: Option<i64>,
}
async fn synced_entities(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(kind): Path<String>,
    Query(q): Query<EntityQuery>,
) -> Answer {
    wire_sync::capture(&s, &a).await?;
    Ok(Json(
        s.service
            .store()
            .android_wire_sync_entities(
                &a,
                &kind,
                q.after,
                q.cursor,
                q.limit.unwrap_or(200).clamp(1, 1000) as i64,
            )
            .await?,
    ))
}
async fn events(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(run): Path<Uuid>,
    headers: HeaderMap,
) -> std::result::Result<Response, Failure> {
    if s.service
        .store()
        .android_private_server_run(&a, run)
        .await?
        .is_some()
    {
        return private_server_events(s, a, run, headers).await;
    }
    if s.service
        .store()
        .android_device_run(&a, run)
        .await?
        .is_some()
    {
        return device_events(s, a, run, headers).await;
    }
    let (value, _, _) = s.service.store().android_run_request(&a, run).await?;
    s.service.store().android_observe_run(&a, &value).await?;
    let after = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    let pending = VecDeque::<Value>::new();
    let source = futures_util::stream::unfold(
        (s, a, run, after, pending, false),
        |(s, a, run, mut after, mut pending, terminal)| async move {
            if terminal && pending.is_empty() {
                return None;
            }
            if pending.is_empty() {
                loop {
                    let (value, _, _) =
                        s.service.store().android_run_request(&a, run).await.ok()?;
                    s.service
                        .store()
                        .android_observe_run(&a, &value)
                        .await
                        .ok()?;
                    let fetched = s
                        .service
                        .store()
                        .android_run_events(&a, run, after)
                        .await
                        .ok()?;
                    if !fetched.is_empty() {
                        pending.extend(fetched);
                        break;
                    }
                    if !matches!(value.status, TeamRunStatus::Queued | TeamRunStatus::Running) {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
            // Revalidate current session/project ACL immediately before each frame.
            let (_value, _, _) = s.service.store().android_run_request(&a, run).await.ok()?;
            let event = pending.pop_front()?;
            after = event["sequence"].as_i64()?;
            let terminal = matches!(
                event["payload"]["state"].as_str(),
                Some("completed" | "failed" | "canceled" | "interrupted")
            );
            let frame = Event::default()
                .id(after.to_string())
                .event("run_event")
                .json_data(event)
                .ok()?;
            Some((
                Ok::<_, Infallible>(frame),
                (s, a, run, after, pending, terminal),
            ))
        },
    );
    Ok(Sse::new(source)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(10)))
        .into_response())
}
async fn artifacts(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(run): Path<Uuid>,
) -> Answer {
    if s.service
        .store()
        .android_private_server_run(&a, run)
        .await?
        .is_some()
    {
        return Ok(Json(json!(
            s.service
                .store()
                .android_private_loop_artifacts(&a, run)
                .await?
        )));
    }
    if s.service
        .store()
        .android_device_run(&a, run)
        .await?
        .is_some()
    {
        return Ok(Json(json!(
            s.service.store().android_device_artifacts(&a, run).await?
        )));
    }
    let mut values = Vec::new();
    for b in s.service.store().android_run_files(&a, run).await? {
        values.push(json!({"schema_version":2,"id":b.id,"run_id":run,"revision":b.revision,"kind":"attachment","media_type":b.media_type,"name":b.name,"digest":b.sha256,"size_bytes":b.size,"metadata":{"adapter":"team_file"},"created_at":iso(b.created_at)?,"deleted_at":null}));
    }
    Ok(Json(json!(values)))
}
async fn download(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((run, blob)): Path<(Uuid, Uuid)>,
) -> std::result::Result<Response, Failure> {
    if s.service
        .store()
        .android_private_server_run(&a, run)
        .await?
        .is_some()
    {
        let bytes = s
            .service
            .store()
            .android_private_loop_artifact_bytes(&a, run, blob, s.keys.as_ref())
            .await?;
        return Ok((
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=private-result",
                ),
            ],
            bytes.to_vec(),
        )
            .into_response());
    }
    if s.service
        .store()
        .android_device_run(&a, run)
        .await?
        .is_some()
    {
        return Ok((
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=device-artifact",
                ),
            ],
            s.service
                .store()
                .android_device_artifact_bytes(&a, run, blob)
                .await?,
        )
            .into_response());
    }
    let (r, _, _) = s.service.store().android_run_request(&a, run).await?;
    if !s
        .service
        .store()
        .android_run_files(&a, run)
        .await?
        .iter()
        .any(|b| b.id == blob)
    {
        return Err(Error::NotFound.into());
    }
    let read = s
        .service
        .store()
        .team_blob_read(&a, r.team_id, r.project_id, blob, None)
        .await?;
    let bytes = s.objects.get(read.version()).await?;
    s.service.store().team_blob_read_finish(&a, &read).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=team-file",
            ),
        ],
        bytes,
    )
        .into_response())
}
async fn device_events(
    s: CompatState,
    a: Principal,
    run: Uuid,
    headers: HeaderMap,
) -> std::result::Result<Response, Failure> {
    let after = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    let stream = futures_util::stream::unfold(
        (s, a, run, after, VecDeque::<Value>::new()),
        |(s, a, run, mut after, mut pending)| async move {
            if pending.is_empty() {
                loop {
                    let value = s.service.store().android_device_run(&a, run).await.ok()??;
                    let fetched = s
                        .service
                        .store()
                        .android_device_events(&a, run, after)
                        .await
                        .ok()?;
                    if !fetched.is_empty() {
                        pending.extend(fetched);
                        break;
                    }
                    if !matches!(
                        value["state"].as_str(),
                        Some("queued" | "waiting_for_snapshot" | "running" | "leased")
                    ) {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
            s.service.store().android_device_run(&a, run).await.ok()??;
            let event = pending.pop_front()?;
            after = event["sequence"].as_i64()?;
            let frame = Event::default()
                .id(after.to_string())
                .event("run_event")
                .json_data(event)
                .ok()?;
            Some((Ok::<_, Infallible>(frame), (s, a, run, after, pending)))
        },
    );
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(10)))
        .into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachmentQuery {
    name: String,
}
async fn private_server_events(
    s: CompatState,
    a: Principal,
    run: Uuid,
    headers: HeaderMap,
) -> std::result::Result<Response, Failure> {
    let after = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    let stream = futures_util::stream::unfold(
        (s, a, run, after, VecDeque::<Value>::new()),
        |(s, a, run, mut after, mut pending)| async move {
            if pending.is_empty() {
                loop {
                    let value = s
                        .service
                        .store()
                        .android_private_server_run(&a, run)
                        .await
                        .ok()??;
                    let events = s
                        .service
                        .store()
                        .android_private_server_events_open(&a, run, after, s.keys.as_ref())
                        .await
                        .ok()?;
                    if !events.is_empty() {
                        pending.extend(events);
                        break;
                    }
                    if !matches!(
                        value["state"].as_str(),
                        Some(
                            "queued"
                                | "waiting_for_snapshot"
                                | "running"
                                | "waiting_for_approval"
                                | "waiting_for_input"
                        )
                    ) {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
            s.service
                .store()
                .android_private_server_run(&a, run)
                .await
                .ok()??;
            let event = pending.pop_front()?;
            after = event["sequence"].as_i64()?;
            let frame = Event::default()
                .id(after.to_string())
                .event("run_event")
                .json_data(event)
                .ok()?;
            Some((Ok::<_, Infallible>(frame), (s, a, run, after, pending)))
        },
    );
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(10)))
        .into_response())
}
async fn upload(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(run): Path<Uuid>,
    Query(q): Query<AttachmentQuery>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Answer {
    if bytes.is_empty() {
        return Err(invalid().into());
    }
    let media = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    if s.service
        .store()
        .android_private_server_run(&a, run)
        .await?
        .is_some()
    {
        let artifact = s
            .service
            .store()
            .android_private_upload_attachment(
                &a,
                run,
                &q.name,
                &media,
                &bytes,
                s.keys.as_ref(),
                s.key_id,
            )
            .await?;
        return Ok(Json(
            json!({"schema_version":2,"accepted":true,"artifact_id":artifact["id"]}),
        ));
    }
    if s.service
        .store()
        .android_device_run(&a, run)
        .await?
        .is_some()
    {
        let artifact = s
            .service
            .store()
            .android_device_upload_attachment(&a, run, &q.name, &media, &bytes)
            .await?;
        return Ok(Json(
            json!({"schema_version":2,"accepted":true,"artifact_id":artifact["id"]}),
        ));
    }
    let (r, _, _) = s.service.store().android_run_request(&a, run).await?;
    let key = format!(
        "android:{}",
        hash(
            serde_json::to_vec(&(run, &q.name, hash(&bytes)))
                .map_err(|_| invalid())?
                .as_slice()
        )
    );
    let plan = s
        .service
        .store()
        .team_blob_prepare_upload(
            &a,
            r.team_id,
            r.project_id,
            TeamBlobUpload {
                idempotency_key: key,
                name: q.name,
                media_type: media,
                blob_id: None,
                expected_revision: None,
            },
            hash(&bytes),
            bytes.len() as u64,
            s.objects.backend_id().into(),
        )
        .await?;
    s.service
        .store()
        .android_bind_file(&a, run, plan.metadata())
        .await?;
    if plan.should_send() {
        for v in plan.versions() {
            if let Err(e) = s.objects.put(v, &bytes).await {
                s.service.store().team_blob_uncertain(&plan).await?;
                return Err(e.into());
            }
        }
        if let Err(e) = s.service.store().team_blob_finish(&a, &plan).await {
            s.service.store().team_blob_uncertain(&plan).await?;
            return Err(e.into());
        }
    }
    if !plan.should_send() && plan.metadata().state != TeamBlobState::Ready {
        return Err(Error::UncertainEffect.into());
    }
    Ok(Json(
        json!({"schema_version":2,"accepted":true,"artifact_id":plan.metadata().id}),
    ))
}
