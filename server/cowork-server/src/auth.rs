use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    body::{to_bytes, Body},
    extract::{Extension, Path, Request, State},
    http::{
        header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, COOKIE, SET_COOKIE, VARY},
        HeaderMap, HeaderValue, StatusCode,
    },
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use cowork_contracts::{
    AcceptInvitationRequest, AuthSessionRecord, AuthTokens, BootstrapAdminRequest,
    CreateInvitationRequest, DisableTotpRequest, InvitationSecret, NativeAuthorizationCode,
    NativeAuthorizationRequest, NativeTokenRequest, PasswordLoginRequest, ReauthenticateRequest,
    ReauthenticationGrant, RefreshSessionRequest, TotpRecoveryCodes, TotpSetupResponse, TotpStatus,
    VerifyTotpRequest, SCHEMA_VERSION,
};
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Serialize;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{error::ApiError, storage::SealedValue, AppState};

const ACCESS_LIFETIME_MINUTES: i64 = 15;
const REFRESH_LIFETIME_DAYS: i64 = 30;
const NATIVE_AUTHORIZATION_LIFETIME_MINUTES: i64 = 5;
const TOTP_STEP_SECONDS: i64 = 30;
const TOTP_DIGITS: u32 = 1_000_000;
const BROWSER_SESSION_HEADER: &str = "x-cowork-session-mode";
const BROWSER_SESSION_MODE: &str = "browser-cookie";
const BROWSER_REFRESH_COOKIE: &str = "__Host-cowork_refresh";
const AUTH_RESPONSE_LIMIT: usize = 1024 * 1024;
type HmacSha1 = Hmac<Sha1>;

#[derive(Debug, Clone)]
pub struct Principal {
    pub user_id: Uuid,
    pub session_id: Option<Uuid>,
    pub bootstrap: bool,
}

#[derive(Debug, Clone)]
pub struct ExecutorPrincipal {
    pub executor_id: Uuid,
    pub credential_id: Uuid,
}

#[derive(Debug, Serialize)]
struct AuthError {
    error: &'static str,
    message: &'static str,
}

/// Converts successful token responses into browser-safe sessions. Native clients keep
/// receiving refresh tokens in the JSON contract, while the same-origin web client gets
/// a rotating, host-only HttpOnly cookie and never sees the refresh token in JavaScript.
pub async fn browser_session_boundary(request: Request<Body>, next: Next) -> Response {
    let browser_mode = is_browser_session(request.headers());
    let authentication_request = request.uri().path().contains("/auth/");
    let logout = request.uri().path().ends_with("/auth/logout");
    let refresh = request.uri().path().ends_with("/auth/browser/refresh");
    let mut response = next.run(request).await;
    if authentication_request {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    if !browser_mode {
        return response;
    }
    response
        .headers_mut()
        .append(VARY, HeaderValue::from_static(BROWSER_SESSION_HEADER));

    if (logout && response.status().is_success())
        || (refresh && response.status() == StatusCode::UNAUTHORIZED)
    {
        response.headers_mut().append(
            SET_COOKIE,
            HeaderValue::from_static(
                "__Host-cowork_refresh=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
            ),
        );
        return response;
    }
    if !response.status().is_success() {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, AUTH_RESPONSE_LIMIT).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!(?error, "failed to inspect browser authentication response");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let Some((sanitized, refresh_token)) = sanitize_browser_tokens(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let cookie = format!(
        "{BROWSER_REFRESH_COOKIE}={refresh_token}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={}",
        REFRESH_LIFETIME_DAYS * 24 * 60 * 60
    );
    let Ok(cookie) = HeaderValue::from_str(&cookie) else {
        tracing::error!("generated refresh cookie contained an invalid header value");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    parts.headers.remove(CONTENT_LENGTH);
    parts.headers.append(SET_COOKIE, cookie);
    Response::from_parts(parts, Body::from(sanitized))
}

fn is_browser_session(headers: &HeaderMap) -> bool {
    headers
        .get(BROWSER_SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == BROWSER_SESSION_MODE)
}

fn sanitize_browser_tokens(body: &[u8]) -> Option<(Vec<u8>, String)> {
    let mut value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let object = value.as_object_mut()?;
    let refresh_token = object.remove("refresh_token")?.as_str()?.to_owned();
    if refresh_token.is_empty() {
        return None;
    }
    let sanitized = serde_json::to_vec(&value).ok()?;
    Some((sanitized, refresh_token))
}

fn browser_refresh_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .map(str::trim)
        .find_map(|cookie| {
            cookie
                .strip_prefix(BROWSER_REFRESH_COOKIE)
                .and_then(|value| value.strip_prefix('='))
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
}

pub async fn require_auth(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let supplied = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let Some(token) = supplied else {
        return unauthorized();
    };
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let is_bootstrap = bool::from(digest.ct_eq(&state.bootstrap_token_digest));
    if is_bootstrap {
        let users =
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users WHERE deleted_at IS NULL")
                .fetch_one(&state.pool)
                .await;
        if matches!(users, Ok(0)) {
            request.extensions_mut().insert(Principal {
                user_id: state.bootstrap_user_id,
                session_id: None,
                bootstrap: true,
            });
            return next.run(request).await;
        }
        return unauthorized();
    }

    let principal = sqlx::query(
        r#"
        SELECT token.user_id, token.session_id
        FROM access_tokens token
        JOIN auth_sessions session ON session.id = token.session_id
        JOIN users account ON account.id = token.user_id
        WHERE token.token_hash = $1
          AND token.revoked_at IS NULL
          AND token.expires_at > now()
          AND session.revoked_at IS NULL
          AND session.expires_at > now()
          AND account.deleted_at IS NULL
        "#,
    )
    .bind(digest.as_slice())
    .fetch_optional(&state.pool)
    .await;
    match principal {
        Ok(Some(row)) => {
            request.extensions_mut().insert(Principal {
                user_id: row.get("user_id"),
                session_id: Some(row.get("session_id")),
                bootstrap: false,
            });
            next.run(request).await
        }
        Ok(None) => unauthorized(),
        Err(error) => {
            tracing::error!(?error, "authentication database lookup failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(AuthError {
                    error: "auth_unavailable",
                    message: "authentication is temporarily unavailable",
                }),
            )
                .into_response()
        }
    }
}

pub async fn require_executor_auth(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let supplied = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let Some(token) = supplied else {
        return unauthorized();
    };
    let digest = opaque_token_hash(token);
    let principal = sqlx::query(
        r#"
        UPDATE executor_credentials credential
        SET last_used_at = now()
        FROM executors executor
        WHERE credential.token_hash = $1
          AND credential.executor_id = executor.id
          AND credential.revoked_at IS NULL
          AND (credential.expires_at IS NULL OR credential.expires_at > now())
          AND NOT executor.draining
        RETURNING credential.id, credential.executor_id
        "#,
    )
    .bind(digest.as_slice())
    .fetch_optional(&state.pool)
    .await;
    match principal {
        Ok(Some(row)) => {
            request.extensions_mut().insert(ExecutorPrincipal {
                executor_id: row.get("executor_id"),
                credential_id: row.get("id"),
            });
            next.run(request).await
        }
        Ok(None) => unauthorized(),
        Err(error) => {
            tracing::error!(?error, "executor authentication database lookup failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(AuthError {
                    error: "auth_unavailable",
                    message: "authentication is temporarily unavailable",
                }),
            )
                .into_response()
        }
    }
}

pub async fn bootstrap_admin(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<BootstrapAdminRequest>,
) -> Result<(StatusCode, Json<AuthTokens>), ApiError> {
    if !principal.bootstrap {
        return Err(ApiError::Conflict(
            "the first administrator has already been created".to_owned(),
        ));
    }
    validate_identity(&request.email, &request.password)?;
    if request.display_name.trim().is_empty() || request.display_name.len() > 200 {
        return Err(ApiError::Unprocessable(
            "display_name must contain 1 to 200 characters".to_owned(),
        ));
    }
    let password_hash = hash_password(request.password).await?;
    let user_id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(7100321)")
        .execute(&mut *tx)
        .await?;
    let existing =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users WHERE deleted_at IS NULL")
            .fetch_one(&mut *tx)
            .await?;
    if existing != 0 {
        return Err(ApiError::Conflict(
            "the first administrator has already been created".to_owned(),
        ));
    }
    sqlx::query(
        r#"
        INSERT INTO users (id, etag, email, display_name, platform_admin, password_hash)
        VALUES ($1, $2, lower($3), $4, TRUE, $5)
        "#,
    )
    .bind(user_id)
    .bind(format!("W/\"{user_id}:1\""))
    .bind(request.email.trim())
    .bind(request.display_name.trim())
    .bind(password_hash)
    .execute(&mut *tx)
    .await?;
    let tokens = create_session_tx(&mut tx, user_id, request.device_id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(tokens)))
}

pub async fn password_login(
    State(state): State<AppState>,
    Json(request): Json<PasswordLoginRequest>,
) -> Result<Json<AuthTokens>, ApiError> {
    validate_identity(&request.email, &request.password)?;
    let row = sqlx::query(
        "SELECT id, password_hash FROM users WHERE lower(email) = lower($1) AND deleted_at IS NULL",
    )
    .bind(request.email.trim())
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::Unauthorized(
            "invalid email or password".to_owned(),
        ));
    };
    let user_id: Uuid = row.get("id");
    let password_hash: Option<String> = row.get("password_hash");
    let Some(password_hash) = password_hash else {
        return Err(ApiError::Unauthorized(
            "invalid email or password".to_owned(),
        ));
    };
    if !verify_password(request.password, password_hash).await? {
        return Err(ApiError::Unauthorized(
            "invalid email or password".to_owned(),
        ));
    }
    verify_second_factor(&state, user_id, request.second_factor.as_deref()).await?;
    let mut tx = state.pool.begin().await?;
    let tokens = create_session_tx(&mut tx, user_id, request.device_id).await?;
    tx.commit().await?;
    Ok(Json(tokens))
}

pub async fn native_authorize(
    State(state): State<AppState>,
    Json(request): Json<NativeAuthorizationRequest>,
) -> Result<Json<NativeAuthorizationCode>, ApiError> {
    validate_identity(&request.email, &request.password)?;
    validate_pkce_challenge(&request.code_challenge, &request.code_challenge_method)?;
    let row = sqlx::query(
        "SELECT id, password_hash FROM users WHERE lower(email) = lower($1) AND deleted_at IS NULL",
    )
    .bind(request.email.trim())
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::Unauthorized(
            "invalid email or password".to_owned(),
        ));
    };
    let user_id: Uuid = row.get("id");
    let password_hash: Option<String> = row.get("password_hash");
    let Some(password_hash) = password_hash else {
        return Err(ApiError::Unauthorized(
            "invalid email or password".to_owned(),
        ));
    };
    if !verify_password(request.password, password_hash).await? {
        return Err(ApiError::Unauthorized(
            "invalid email or password".to_owned(),
        ));
    }
    verify_second_factor(&state, user_id, request.second_factor.as_deref()).await?;

    let mut tx = state.pool.begin().await?;
    let authorization = create_native_authorization_code_tx(
        &mut tx,
        user_id,
        request.device_id,
        request.code_challenge,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(authorization))
}

pub(crate) async fn create_native_authorization_code_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    device_id: Uuid,
    code_challenge: String,
) -> Result<NativeAuthorizationCode, ApiError> {
    let code = random_token()?;
    let code_hash = opaque_token_hash(&code);
    let expires_at = Utc::now() + chrono::Duration::minutes(NATIVE_AUTHORIZATION_LIFETIME_MINUTES);
    sqlx::query(
        r#"
        INSERT INTO native_authorization_codes
            (code_hash, user_id, device_id, code_challenge, expires_at)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(code_hash.as_slice())
    .bind(user_id)
    .bind(device_id)
    .bind(code_challenge)
    .bind(expires_at)
    .execute(&mut **tx)
    .await?;
    Ok(NativeAuthorizationCode {
        schema_version: SCHEMA_VERSION,
        code,
        expires_at,
    })
}

pub async fn native_token(
    State(state): State<AppState>,
    Json(request): Json<NativeTokenRequest>,
) -> Result<Json<AuthTokens>, ApiError> {
    validate_pkce_verifier(&request.code_verifier)?;
    let code_hash = opaque_token_hash(&request.code);
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        SELECT user_id, device_id, code_challenge, expires_at
        FROM native_authorization_codes
        WHERE code_hash = $1 AND consumed_at IS NULL
        FOR UPDATE
        "#,
    )
    .bind(code_hash.as_slice())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::Unauthorized(
            "authorization code is invalid or already used".to_owned(),
        ));
    };

    // Consume first. A wrong verifier or device binding must burn the code,
    // preventing repeated probing of a captured native authorization code.
    sqlx::query("UPDATE native_authorization_codes SET consumed_at = now() WHERE code_hash = $1")
        .bind(code_hash.as_slice())
        .execute(&mut *tx)
        .await?;

    let user_id: Uuid = row.get("user_id");
    let bound_device_id: Uuid = row.get("device_id");
    let expected_challenge: String = row.get("code_challenge");
    let expires_at: DateTime<Utc> = row.get("expires_at");
    let actual_challenge = pkce_challenge(&request.code_verifier);
    let valid = expires_at > Utc::now()
        && request.device_id == bound_device_id
        && bool::from(
            actual_challenge
                .as_bytes()
                .ct_eq(expected_challenge.as_bytes()),
        );
    if !valid {
        tx.commit().await?;
        return Err(ApiError::Unauthorized(
            "authorization code, verifier, or device binding is invalid".to_owned(),
        ));
    }

    let tokens = create_session_tx(&mut tx, user_id, request.device_id).await?;
    tx.commit().await?;
    Ok(Json(tokens))
}

pub async fn create_invitation(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateInvitationRequest>,
) -> Result<(StatusCode, Json<InvitationSecret>), ApiError> {
    let is_admin = sqlx::query_scalar::<_, bool>(
        "SELECT platform_admin FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(principal.user_id)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or(false);
    if !is_admin {
        return Err(ApiError::Unauthorized(
            "only platform administrators can create invitations".to_owned(),
        ));
    }
    validate_email(&request.email)?;
    let now = Utc::now();
    let expires_at = request
        .expires_at
        .unwrap_or_else(|| now + chrono::Duration::days(7));
    if expires_at <= now + chrono::Duration::hours(1)
        || expires_at > now + chrono::Duration::days(30)
    {
        return Err(ApiError::Unprocessable(
            "invitations must expire between one hour and 30 days from now".to_owned(),
        ));
    }
    let token = random_token()?;
    let digest = opaque_token_hash(&token);
    let invitation_id = Uuid::new_v4();
    let email = request.email.trim().to_ascii_lowercase();
    sqlx::query(
        r#"
        INSERT INTO invitations (id, email, token_hash, invited_by, expires_at)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(invitation_id)
    .bind(&email)
    .bind(digest.as_slice())
    .bind(principal.user_id)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(InvitationSecret {
            schema_version: SCHEMA_VERSION,
            invitation_id,
            email,
            token,
            expires_at,
        }),
    ))
}

pub async fn accept_invitation(
    State(state): State<AppState>,
    Json(request): Json<AcceptInvitationRequest>,
) -> Result<(StatusCode, Json<AuthTokens>), ApiError> {
    if request.display_name.trim().is_empty() || request.display_name.len() > 200 {
        return Err(ApiError::Unprocessable(
            "display_name must contain 1 to 200 characters".to_owned(),
        ));
    }
    let digest = opaque_token_hash(&request.token);
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        SELECT id, email FROM invitations
        WHERE token_hash = $1 AND accepted_at IS NULL AND expires_at > now()
        FOR UPDATE
        "#,
    )
    .bind(digest.as_slice())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::Unauthorized("invitation is invalid or expired".to_owned()))?;
    let invitation_id: Uuid = row.get("id");
    let email: String = row.get("email");
    validate_identity(&email, &request.password)?;
    let password_hash = hash_password(request.password).await?;
    let user_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO users (id, etag, email, display_name, password_hash)
        VALUES ($1, $2, lower($3), $4, $5)
        "#,
    )
    .bind(user_id)
    .bind(format!("W/\"{user_id}:1\""))
    .bind(&email)
    .bind(request.display_name.trim())
    .bind(password_hash)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE invitations SET accepted_by = $2, accepted_at = now() WHERE id = $1")
        .bind(invitation_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let tokens = create_session_tx(&mut tx, user_id, request.device_id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(tokens)))
}

pub async fn refresh_session(
    State(state): State<AppState>,
    Json(request): Json<RefreshSessionRequest>,
) -> Result<Json<AuthTokens>, ApiError> {
    Ok(Json(
        rotate_refresh_session(&state, &request.refresh_token).await?,
    ))
}

pub async fn browser_refresh_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AuthTokens>, ApiError> {
    if !is_browser_session(&headers) {
        return Err(ApiError::Unauthorized(
            "browser refresh requires the browser-cookie session mode".to_owned(),
        ));
    }
    let refresh_token = browser_refresh_token(&headers).ok_or_else(|| {
        ApiError::Unauthorized("browser session cookie is missing or expired".to_owned())
    })?;
    Ok(Json(rotate_refresh_session(&state, &refresh_token).await?))
}

async fn rotate_refresh_session(
    state: &AppState,
    refresh_token: &str,
) -> Result<AuthTokens, ApiError> {
    let supplied_hash = token_hash(refresh_token);
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        SELECT id, user_id, device_id, refresh_family_id, expires_at
        FROM auth_sessions
        WHERE refresh_token_hash = $1 AND revoked_at IS NULL
        FOR UPDATE
        "#,
    )
    .bind(supplied_hash.as_slice())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        // A previous token being presented again indicates theft/replay. Revoke
        // the complete rotation family rather than accepting either copy.
        if let Some(family_id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT refresh_family_id FROM auth_refresh_token_history WHERE token_hash = $1 LIMIT 1",
        )
        .bind(supplied_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            sqlx::query(
                "UPDATE auth_sessions SET revoked_at = now(), revoke_reason = 'refresh_token_reuse' WHERE refresh_family_id = $1 AND revoked_at IS NULL",
            )
            .bind(family_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
        }
        return Err(ApiError::Unauthorized(
            "refresh token is invalid or expired".to_owned(),
        ));
    };
    let session_id: Uuid = row.get("id");
    let user_id: Uuid = row.get("user_id");
    let refresh_family_id: Uuid = row.get("refresh_family_id");
    let expires_at: DateTime<Utc> = row.get("expires_at");
    if expires_at <= Utc::now() {
        return Err(ApiError::Unauthorized(
            "refresh token is invalid or expired".to_owned(),
        ));
    }
    let refresh_token = random_token()?;
    let refresh_hash = token_hash(&refresh_token);
    sqlx::query(
        r#"
        INSERT INTO auth_refresh_token_history
            (token_hash, session_id, refresh_family_id)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(supplied_hash.as_slice())
    .bind(session_id)
    .bind(refresh_family_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE auth_sessions SET previous_token_hash = refresh_token_hash,
            refresh_token_hash = $2, last_used_at = now()
        WHERE id = $1
        "#,
    )
    .bind(session_id)
    .bind(refresh_hash.as_slice())
    .execute(&mut *tx)
    .await?;
    let (access_token, access_expires_at) =
        issue_access_token(&mut tx, user_id, session_id).await?;
    tx.commit().await?;
    Ok(AuthTokens {
        schema_version: SCHEMA_VERSION,
        access_token,
        access_expires_at,
        refresh_token,
        refresh_expires_at: expires_at,
        user_id,
        session_id,
    })
}

pub async fn logout(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, ApiError> {
    let Some(session_id) = principal.session_id else {
        return Err(ApiError::Conflict(
            "bootstrap authentication does not have a session".to_owned(),
        ));
    };
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "UPDATE auth_sessions SET revoked_at = now(), revoke_reason = 'user_logout' WHERE id = $1",
    )
    .bind(session_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE access_tokens SET revoked_at = now() WHERE session_id = $1")
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_sessions(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<AuthSessionRecord>>, ApiError> {
    let current_session_id = principal.session_id.ok_or_else(|| {
        ApiError::Unauthorized("session listing requires a signed-in user session".to_owned())
    })?;
    let rows = sqlx::query(
        r#"
        SELECT id, device_id, created_at, last_used_at, expires_at, revoked_at, revoke_reason,
            id = $2 AS current,
            revoked_at IS NULL AND expires_at > now() AS active
        FROM auth_sessions
        WHERE user_id = $1 AND created_at > now() - interval '90 days'
        ORDER BY (id = $2) DESC, active DESC, last_used_at DESC
        LIMIT 100
        "#,
    )
    .bind(principal.user_id)
    .bind(current_session_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| AuthSessionRecord {
                schema_version: SCHEMA_VERSION,
                id: row.get("id"),
                device_id: row.get("device_id"),
                current: row.get("current"),
                active: row.get("active"),
                created_at: row.get("created_at"),
                last_used_at: row.get("last_used_at"),
                expires_at: row.get("expires_at"),
                revoked_at: row.get("revoked_at"),
                revoke_reason: row.get("revoke_reason"),
            })
            .collect(),
    ))
}

pub async fn revoke_session(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(session_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let current_session_id = principal.session_id.ok_or_else(|| {
        ApiError::Unauthorized("session revocation requires a signed-in user session".to_owned())
    })?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        "SELECT device_id, revoked_at FROM auth_sessions WHERE id = $1 AND user_id = $2 FOR UPDATE",
    )
    .bind(session_id)
    .bind(principal.user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::NotFound("authentication session was not found".to_owned()))?;
    let device_id: Uuid = row.get("device_id");
    let revoked_at: Option<DateTime<Utc>> = row.get("revoked_at");
    if revoked_at.is_none() {
        sqlx::query(
            "UPDATE auth_sessions SET revoked_at = now(), revoke_reason = 'user_device_revoked' WHERE id = $1",
        )
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE access_tokens SET revoked_at = now() WHERE session_id = $1")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            r#"
            DELETE FROM push_subscriptions subscription
            WHERE subscription.user_id = $1 AND subscription.device_id = $2
              AND NOT EXISTS (
                SELECT 1 FROM auth_sessions session
                WHERE session.user_id = $1 AND session.device_id = $2
                  AND session.revoked_at IS NULL AND session.expires_at > now()
              )
            "#,
        )
        .bind(principal.user_id)
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, 'auth.session.revoked', 'auth_session', $3, $4)",
        )
        .bind(Uuid::new_v4())
        .bind(principal.user_id)
        .bind(session_id)
        .bind(serde_json::json!({
            "device_id": device_id,
            "current": session_id == current_session_id,
        }))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn reauthenticate(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<ReauthenticateRequest>,
) -> Result<Json<ReauthenticationGrant>, ApiError> {
    let session_id = principal.session_id.ok_or_else(|| {
        ApiError::Unauthorized("reauthentication requires a signed-in user session".to_owned())
    })?;
    if request.purpose != "desktop_control" {
        return Err(ApiError::Unprocessable(
            "unsupported reauthentication purpose".to_owned(),
        ));
    }
    let encoded = sqlx::query_scalar::<_, Option<String>>(
        "SELECT password_hash FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(principal.user_id)
    .fetch_optional(&state.pool)
    .await?
    .flatten()
    .ok_or_else(|| {
        ApiError::Unauthorized("this account cannot reauthenticate with a password".to_owned())
    })?;
    if !verify_password(request.password, encoded).await? {
        return Err(ApiError::Unauthorized(
            "password reauthentication failed".to_owned(),
        ));
    }
    let token = random_token()?;
    let digest = token_hash(&token);
    let expires_at = Utc::now() + chrono::Duration::minutes(5);
    sqlx::query(
        "INSERT INTO reauthentication_grants (token_hash, user_id, session_id, purpose, expires_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(digest.as_slice())
    .bind(principal.user_id)
    .bind(session_id)
    .bind(&request.purpose)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;
    Ok(Json(ReauthenticationGrant {
        schema_version: SCHEMA_VERSION,
        token,
        purpose: request.purpose,
        expires_at,
    }))
}

pub(crate) async fn create_session_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    device_id: Uuid,
) -> Result<AuthTokens, ApiError> {
    let session_id = Uuid::new_v4();
    let refresh_family_id = Uuid::new_v4();
    let refresh_token = random_token()?;
    let refresh_hash = token_hash(&refresh_token);
    let refresh_expires_at = Utc::now() + chrono::Duration::days(REFRESH_LIFETIME_DAYS);
    sqlx::query(
        r#"
        INSERT INTO auth_sessions (
            id, user_id, device_id, refresh_token_hash, refresh_family_id, expires_at
        ) VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(session_id)
    .bind(user_id)
    .bind(device_id)
    .bind(refresh_hash.as_slice())
    .bind(refresh_family_id)
    .bind(refresh_expires_at)
    .execute(&mut **tx)
    .await?;
    let (access_token, access_expires_at) = issue_access_token(tx, user_id, session_id).await?;
    Ok(AuthTokens {
        schema_version: SCHEMA_VERSION,
        access_token,
        access_expires_at,
        refresh_token,
        refresh_expires_at,
        user_id,
        session_id,
    })
}

async fn issue_access_token(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<(String, DateTime<Utc>), ApiError> {
    let access_token = random_token()?;
    let hash = token_hash(&access_token);
    let expires_at = Utc::now() + chrono::Duration::minutes(ACCESS_LIFETIME_MINUTES);
    sqlx::query(
        "INSERT INTO access_tokens (token_hash, session_id, user_id, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(hash.as_slice())
    .bind(session_id)
    .bind(user_id)
    .bind(expires_at)
    .execute(&mut **tx)
    .await?;
    Ok((access_token, expires_at))
}

fn validate_identity(email: &str, password: &str) -> Result<(), ApiError> {
    validate_email(email)?;
    if !(12..=1024).contains(&password.chars().count()) {
        return Err(ApiError::Unprocessable(
            "password must contain 12 to 1024 characters".to_owned(),
        ));
    }
    Ok(())
}

fn validate_email(email: &str) -> Result<(), ApiError> {
    let email = email.trim();
    if email.len() > 320 || !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err(ApiError::Unprocessable("email is invalid".to_owned()));
    }
    Ok(())
}

pub(crate) fn validate_email_for_external_identity(email: &str) -> Result<(), ApiError> {
    validate_email(email)
}

pub(crate) fn validate_pkce_challenge(challenge: &str, method: &str) -> Result<(), ApiError> {
    if method != "S256" {
        return Err(ApiError::Unprocessable(
            "code_challenge_method must be S256".to_owned(),
        ));
    }
    let decoded = URL_SAFE_NO_PAD.decode(challenge).map_err(|_| {
        ApiError::Unprocessable("code_challenge must be base64url without padding".to_owned())
    })?;
    if challenge.len() != 43 || decoded.len() != 32 {
        return Err(ApiError::Unprocessable(
            "code_challenge must contain a 256-bit S256 digest".to_owned(),
        ));
    }
    Ok(())
}

fn validate_pkce_verifier(verifier: &str) -> Result<(), ApiError> {
    if !(43..=128).contains(&verifier.len())
        || !verifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
    {
        return Err(ApiError::Unprocessable(
            "code_verifier must be 43 to 128 unreserved ASCII characters".to_owned(),
        ));
    }
    Ok(())
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub async fn totp_status(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<TotpStatus>, ApiError> {
    let enabled = sqlx::query_scalar::<_, bool>("SELECT enabled FROM user_totp WHERE user_id = $1")
        .bind(principal.user_id)
        .fetch_optional(&state.pool)
        .await?
        .unwrap_or(false);
    let unused = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM recovery_codes WHERE user_id = $1 AND used_at IS NULL",
    )
    .bind(principal.user_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(TotpStatus {
        schema_version: SCHEMA_VERSION,
        enabled,
        unused_recovery_codes: u16::try_from(unused).unwrap_or(u16::MAX),
    }))
}

pub async fn setup_totp(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<TotpSetupResponse>, ApiError> {
    if principal.session_id.is_none() {
        return Err(ApiError::Unauthorized(
            "TOTP setup requires a user session".to_owned(),
        ));
    }
    let store = state.object_store.as_ref().ok_or_else(|| {
        ApiError::Conflict("encrypted secret storage is not configured".to_owned())
    })?;
    let already_enabled =
        sqlx::query_scalar::<_, bool>("SELECT enabled FROM user_totp WHERE user_id = $1")
            .bind(principal.user_id)
            .fetch_optional(&state.pool)
            .await?
            .unwrap_or(false);
    if already_enabled {
        return Err(ApiError::Conflict("TOTP is already enabled".to_owned()));
    }
    let mut secret = [0_u8; 20];
    getrandom::fill(&mut secret).map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    let sealed = store.seal_for_user(principal.user_id, &secret)?;
    let expires_at = Utc::now() + chrono::Duration::minutes(10);
    sqlx::query(
        r#"
        INSERT INTO user_totp (
            user_id, ciphertext, encrypted_data_key, nonce, wrap_nonce,
            enabled, pending_expires_at
        ) VALUES ($1, $2, $3, $4, $5, FALSE, $6)
        ON CONFLICT (user_id) DO UPDATE SET
            ciphertext = EXCLUDED.ciphertext,
            encrypted_data_key = EXCLUDED.encrypted_data_key,
            nonce = EXCLUDED.nonce,
            wrap_nonce = EXCLUDED.wrap_nonce,
            pending_expires_at = EXCLUDED.pending_expires_at,
            last_used_step = NULL
        WHERE NOT user_totp.enabled
        "#,
    )
    .bind(principal.user_id)
    .bind(sealed.ciphertext)
    .bind(sealed.encrypted_data_key)
    .bind(sealed.nonce.as_slice())
    .bind(sealed.wrap_nonce.as_slice())
    .bind(expires_at)
    .execute(&state.pool)
    .await?;
    let email = sqlx::query_scalar::<_, String>("SELECT email FROM users WHERE id = $1")
        .bind(principal.user_id)
        .fetch_one(&state.pool)
        .await?;
    let encoded_secret = base32_encode(&secret);
    let account_label = format!("Open Cowork:{email}");
    let label = utf8_percent_encode(&account_label, NON_ALPHANUMERIC);
    let issuer = utf8_percent_encode("Open Cowork", NON_ALPHANUMERIC);
    Ok(Json(TotpSetupResponse {
        schema_version: SCHEMA_VERSION,
        secret: encoded_secret.clone(),
        otpauth_uri: format!(
            "otpauth://totp/{label}?secret={encoded_secret}&issuer={issuer}&algorithm=SHA1&digits=6&period=30"
        ),
        expires_at,
    }))
}

pub async fn enable_totp(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<VerifyTotpRequest>,
) -> Result<Json<TotpRecoveryCodes>, ApiError> {
    let store = state.object_store.as_ref().ok_or_else(|| {
        ApiError::Conflict("encrypted secret storage is not configured".to_owned())
    })?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        "SELECT * FROM user_totp WHERE user_id = $1 AND NOT enabled AND pending_expires_at > now() FOR UPDATE",
    )
    .bind(principal.user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::Conflict("TOTP setup is missing or expired".to_owned()))?;
    let sealed = sealed_from_row(&row)?;
    let secret = store.open_for_user(principal.user_id, &sealed)?;
    let step = matching_totp_step(&secret, &request.code, Utc::now().timestamp())
        .ok_or_else(|| ApiError::Unauthorized("TOTP code is invalid".to_owned()))?;
    let recovery_codes = generate_recovery_codes()?;
    sqlx::query("DELETE FROM recovery_codes WHERE user_id = $1")
        .bind(principal.user_id)
        .execute(&mut *tx)
        .await?;
    insert_recovery_codes(&mut tx, principal.user_id, &recovery_codes).await?;
    sqlx::query(
        "UPDATE user_totp SET enabled = TRUE, enabled_at = now(), pending_expires_at = NULL, last_used_step = $2 WHERE user_id = $1",
    )
    .bind(principal.user_id)
    .bind(step)
    .execute(&mut *tx)
    .await?;
    insert_auth_audit(&mut tx, principal.user_id, "auth.totp.enabled").await?;
    tx.commit().await?;
    Ok(Json(TotpRecoveryCodes {
        schema_version: SCHEMA_VERSION,
        recovery_codes,
    }))
}

pub async fn regenerate_recovery_codes(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<VerifyTotpRequest>,
) -> Result<Json<TotpRecoveryCodes>, ApiError> {
    verify_second_factor(&state, principal.user_id, Some(&request.code)).await?;
    let codes = generate_recovery_codes()?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM recovery_codes WHERE user_id = $1")
        .bind(principal.user_id)
        .execute(&mut *tx)
        .await?;
    insert_recovery_codes(&mut tx, principal.user_id, &codes).await?;
    insert_auth_audit(
        &mut tx,
        principal.user_id,
        "auth.recovery_codes.regenerated",
    )
    .await?;
    tx.commit().await?;
    Ok(Json(TotpRecoveryCodes {
        schema_version: SCHEMA_VERSION,
        recovery_codes: codes,
    }))
}

pub async fn disable_totp(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<DisableTotpRequest>,
) -> Result<StatusCode, ApiError> {
    let password_hash = sqlx::query_scalar::<_, Option<String>>(
        "SELECT password_hash FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(principal.user_id)
    .fetch_optional(&state.pool)
    .await?
    .flatten()
    .ok_or_else(|| ApiError::Unauthorized("password verification is unavailable".to_owned()))?;
    if !verify_password(request.password, password_hash).await? {
        return Err(ApiError::Unauthorized("password is invalid".to_owned()));
    }
    verify_second_factor(&state, principal.user_id, Some(&request.second_factor)).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM user_totp WHERE user_id = $1")
        .bind(principal.user_id)
        .execute(&mut *tx)
        .await?;
    insert_auth_audit(&mut tx, principal.user_id, "auth.totp.disabled").await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn verify_second_factor(
    state: &AppState,
    user_id: Uuid,
    supplied: Option<&str>,
) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT * FROM user_totp WHERE user_id = $1 AND enabled FOR UPDATE")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(());
    };
    let supplied = supplied
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::Unauthorized("TOTP or recovery code is required".to_owned()))?;
    let mut accepted = false;
    if supplied.len() == 6 && supplied.bytes().all(|byte| byte.is_ascii_digit()) {
        let store = state.object_store.as_ref().ok_or_else(|| {
            ApiError::Internal(anyhow::anyhow!("encrypted TOTP storage is unavailable"))
        })?;
        let secret = store.open_for_user(user_id, &sealed_from_row(&row)?)?;
        if let Some(step) = matching_totp_step(&secret, supplied, Utc::now().timestamp()) {
            let last_used: Option<i64> = row.try_get("last_used_step")?;
            if last_used.is_none_or(|prior| step > prior) {
                sqlx::query("UPDATE user_totp SET last_used_step = $2 WHERE user_id = $1")
                    .bind(user_id)
                    .bind(step)
                    .execute(&mut *tx)
                    .await?;
                accepted = true;
            }
        }
    } else if let Some(normalized) = normalize_recovery_code(supplied) {
        let digest = opaque_token_hash(&normalized);
        accepted = sqlx::query(
            "UPDATE recovery_codes SET used_at = now() WHERE user_id = $1 AND code_hash = $2 AND used_at IS NULL RETURNING id",
        )
        .bind(user_id)
        .bind(digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
    }
    if !accepted {
        return Err(ApiError::Unauthorized(
            "TOTP or recovery code is invalid or already used".to_owned(),
        ));
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn verify_account_password_and_second_factor(
    state: &AppState,
    user_id: Uuid,
    password: String,
    second_factor: Option<&str>,
) -> Result<(), ApiError> {
    let encoded = sqlx::query_scalar::<_, Option<String>>(
        "SELECT password_hash FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?
    .flatten()
    .ok_or_else(|| ApiError::Unauthorized("password verification is unavailable".to_owned()))?;
    if !verify_password(password, encoded).await? {
        return Err(ApiError::Unauthorized("password is invalid".to_owned()));
    }
    verify_second_factor(state, user_id, second_factor).await
}

fn sealed_from_row(row: &sqlx::postgres::PgRow) -> Result<SealedValue, ApiError> {
    let nonce: Vec<u8> = row.try_get("nonce")?;
    let wrap_nonce: Vec<u8> = row.try_get("wrap_nonce")?;
    Ok(SealedValue {
        ciphertext: row.try_get("ciphertext")?,
        encrypted_data_key: row.try_get("encrypted_data_key")?,
        nonce: nonce
            .try_into()
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid TOTP nonce")))?,
        wrap_nonce: wrap_nonce
            .try_into()
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid TOTP wrap nonce")))?,
    })
}

fn matching_totp_step(secret: &[u8], supplied: &str, unix_seconds: i64) -> Option<i64> {
    let current = unix_seconds.div_euclid(TOTP_STEP_SECONDS);
    [current - 1, current, current + 1]
        .into_iter()
        .find(|step| {
            let expected = format!("{:06}", totp_at(secret, *step as u64));
            bool::from(expected.as_bytes().ct_eq(supplied.as_bytes()))
        })
}

fn totp_at(secret: &[u8], step: u64) -> u32 {
    let mut mac = HmacSha1::new_from_slice(secret).expect("HMAC accepts arbitrary key lengths");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[19] & 0x0f);
    let binary = (u32::from(digest[offset] & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    binary % TOTP_DIGITS
}

fn base32_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut output = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let mut buffer = 0_u32;
    let mut bits = 0_u8;
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            output.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        output.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    output
}

fn generate_recovery_codes() -> Result<Vec<String>, ApiError> {
    (0..10)
        .map(|_| {
            let mut bytes = [0_u8; 10];
            getrandom::fill(&mut bytes)
                .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
            let raw = base32_encode(&bytes);
            Ok(raw
                .as_bytes()
                .chunks(4)
                .map(|chunk| std::str::from_utf8(chunk).expect("base32 is ASCII"))
                .collect::<Vec<_>>()
                .join("-"))
        })
        .collect()
}

fn normalize_recovery_code(value: &str) -> Option<String> {
    let normalized = value
        .chars()
        .filter(|character| *character != '-' && !character.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    (normalized.len() == 16
        && normalized
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || matches!(byte, b'2'..=b'7')))
    .then_some(normalized)
}

async fn insert_recovery_codes(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    codes: &[String],
) -> Result<(), ApiError> {
    for code in codes {
        let normalized = normalize_recovery_code(code).ok_or_else(|| {
            ApiError::Internal(anyhow::anyhow!("invalid generated recovery code"))
        })?;
        let digest = opaque_token_hash(&normalized);
        sqlx::query("INSERT INTO recovery_codes (id, user_id, code_hash) VALUES ($1, $2, $3)")
            .bind(Uuid::new_v4())
            .bind(user_id)
            .bind(digest.as_slice())
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn insert_auth_audit(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    action: &str,
) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id) VALUES ($1, $2, $3, 'user', $2)")
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(action)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn hash_password(password: String) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|error| ApiError::Internal(anyhow::anyhow!(error.to_string())))
    })
    .await
    .map_err(|error| ApiError::Internal(error.into()))?
}

async fn verify_password(password: String, encoded: String) -> Result<bool, ApiError> {
    tokio::task::spawn_blocking(move || {
        let hash = PasswordHash::new(&encoded)
            .map_err(|error| ApiError::Internal(anyhow::anyhow!(error.to_string())))?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok())
    })
    .await
    .map_err(|error| ApiError::Internal(error.into()))?
}

pub(crate) fn random_token() -> Result<String, ApiError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn token_hash(token: &str) -> [u8; 32] {
    opaque_token_hash(token)
}

pub(crate) fn opaque_token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(AuthError {
            error: "unauthorized",
            message: "a valid bearer token is required",
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_rfc_7636_s256_example() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(pkce_challenge(verifier), challenge);
        assert!(validate_pkce_verifier(verifier).is_ok());
        assert!(validate_pkce_challenge(challenge, "S256").is_ok());
    }

    #[test]
    fn rejects_plain_or_malformed_pkce_values() {
        assert!(validate_pkce_challenge("x", "plain").is_err());
        assert!(validate_pkce_challenge("x", "S256").is_err());
        assert!(validate_pkce_verifier("short").is_err());
        assert!(validate_pkce_verifier(&"a".repeat(129)).is_err());
        assert!(validate_pkce_verifier(&format!("{}!", "a".repeat(42))).is_err());
    }

    #[test]
    fn implements_rfc_6238_sha1_vectors_with_six_digits() {
        let secret = b"12345678901234567890";
        assert_eq!(totp_at(secret, 59 / 30), 287_082);
        assert_eq!(totp_at(secret, 1_111_111_109 / 30), 81_804);
        assert_eq!(totp_at(secret, 1_234_567_890 / 30), 5_924);
        assert_eq!(totp_at(secret, 2_000_000_000 / 30), 279_037);
    }

    #[test]
    fn accepts_only_the_adjacent_totp_window() {
        let secret = b"12345678901234567890";
        let now = 1_234_567_890_i64;
        let code = format!("{:06}", totp_at(secret, (now / 30) as u64));
        assert_eq!(matching_totp_step(secret, &code, now), Some(now / 30));
        assert_eq!(matching_totp_step(secret, &code, now + 90), None);
    }

    #[test]
    fn recovery_codes_normalize_without_weakening_the_alphabet() {
        assert_eq!(
            normalize_recovery_code("abcd-efgh-jk23-4567"),
            Some("ABCDEFGHJK234567".to_owned())
        );
        assert_eq!(normalize_recovery_code("ABCD-EFGH-IJKL-MNO0"), None);
        assert_eq!(base32_encode(b"foo"), "MZXW6");
    }

    #[test]
    fn browser_boundary_removes_refresh_token_from_json() {
        let input = serde_json::json!({
            "schema_version": 2,
            "access_token": "visible-access-token",
            "refresh_token": "secret-refresh-token",
            "refresh_expires_at": "2026-09-08T12:00:00Z"
        });
        let (sanitized, refresh_token) =
            sanitize_browser_tokens(&serde_json::to_vec(&input).unwrap()).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&sanitized).unwrap();
        assert_eq!(refresh_token, "secret-refresh-token");
        assert_eq!(value["access_token"], "visible-access-token");
        assert!(value.get("refresh_token").is_none());
    }

    #[test]
    fn browser_refresh_cookie_is_host_only_and_parsed_exactly() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("theme=dark; __Host-cowork_refresh=abc_123-XYZ; other=value"),
        );
        assert_eq!(
            browser_refresh_token(&headers).as_deref(),
            Some("abc_123-XYZ")
        );

        headers.insert(
            COOKIE,
            HeaderValue::from_static("cowork_refresh=wrong; __Host-cowork_refresh_extra=wrong"),
        );
        assert!(browser_refresh_token(&headers).is_none());
    }
}
