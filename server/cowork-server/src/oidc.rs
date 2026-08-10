use axum::{
    extract::{Query, State},
    response::Redirect,
    Json,
};
use chrono::Utc;
use cowork_contracts::{
    OidcAuthorization, OidcConfiguration, StartOidcAuthorizationRequest, SCHEMA_VERSION,
};
use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
    AccessTokenHash, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
use reqwest::{redirect::Policy, Client, Url};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    auth::{self, Principal},
    config::OidcConfig,
    error::ApiError,
    AppState,
};

const AUTHORIZATION_LIFETIME_MINUTES: i64 = 5;
const NATIVE_CALLBACK: &str = "open-cowork://auth/callback";

#[derive(Clone)]
pub struct OidcService {
    metadata: CoreProviderMetadata,
    client_id: String,
    client_secret: String,
    issuer: String,
    provider_callback: String,
    web_callback: String,
    auto_provision: bool,
    http: Client,
}

#[derive(Debug, Deserialize)]
pub struct OidcCallbackQuery {
    code: Option<String>,
    state: String,
    error: Option<String>,
}

pub async fn build(config: Option<&OidcConfig>) -> anyhow::Result<Option<OidcService>> {
    let Some(config) = config else {
        return Ok(None);
    };
    let issuer = validated_https_or_loopback_url(&config.issuer, "COWORK_OIDC_ISSUER")?;
    if issuer.query().is_some() || issuer.fragment().is_some() {
        anyhow::bail!("COWORK_OIDC_ISSUER cannot contain a query or fragment");
    }
    let public_origin =
        validated_https_or_loopback_url(&config.public_origin, "COWORK_PUBLIC_ORIGIN")?;
    if public_origin.path() != "/"
        || public_origin.query().is_some()
        || public_origin.fragment().is_some()
    {
        anyhow::bail!("COWORK_PUBLIC_ORIGIN must be an origin without a path, query, or fragment");
    }
    let public_origin = public_origin.as_str().trim_end_matches('/');
    let http = Client::builder().redirect(Policy::none()).build()?;
    let configured_issuer = config.issuer.trim().to_owned();
    let issuer_url = IssuerUrl::new(configured_issuer.clone())?;
    let metadata = CoreProviderMetadata::discover_async(issuer_url, &http).await?;
    if metadata.issuer().as_str() != configured_issuer {
        anyhow::bail!("OIDC discovery issuer does not exactly match COWORK_OIDC_ISSUER");
    }
    Ok(Some(OidcService {
        metadata,
        client_id: config.client_id.clone(),
        client_secret: config.client_secret.clone(),
        issuer: configured_issuer,
        provider_callback: format!("{public_origin}/api/v1/auth/oidc/callback"),
        web_callback: format!("{public_origin}/auth/callback"),
        auto_provision: config.auto_provision,
        http,
    }))
}

pub async fn configuration(State(state): State<AppState>) -> Json<OidcConfiguration> {
    Json(OidcConfiguration {
        schema_version: SCHEMA_VERSION,
        enabled: state.oidc.is_some(),
    })
}

pub async fn start(
    State(state): State<AppState>,
    Json(request): Json<StartOidcAuthorizationRequest>,
) -> Result<Json<OidcAuthorization>, ApiError> {
    start_authorization(&state, request, None).await
}

pub async fn start_link(
    State(state): State<AppState>,
    axum::extract::Extension(principal): axum::extract::Extension<Principal>,
    Json(request): Json<StartOidcAuthorizationRequest>,
) -> Result<Json<OidcAuthorization>, ApiError> {
    if principal.session_id.is_none() {
        return Err(ApiError::Unauthorized(
            "linking an OIDC identity requires a user session".to_owned(),
        ));
    }
    start_authorization(&state, request, Some(principal.user_id)).await
}

async fn start_authorization(
    state: &AppState,
    request: StartOidcAuthorizationRequest,
    link_user_id: Option<Uuid>,
) -> Result<Json<OidcAuthorization>, ApiError> {
    let service = state
        .oidc
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("OIDC is not configured".to_owned()))?;
    auth::validate_pkce_challenge(&request.code_challenge, &request.code_challenge_method)?;
    validate_client_state(&request.client_state)?;
    if request.redirect_uri != service.web_callback && request.redirect_uri != NATIVE_CALLBACK {
        return Err(ApiError::Unprocessable(
            "redirect_uri is not an allowed OIDC client callback".to_owned(),
        ));
    }

    let client = CoreClient::from_provider_metadata(
        service.metadata.clone(),
        ClientId::new(service.client_id.clone()),
        Some(ClientSecret::new(service.client_secret.clone())),
    )
    .set_redirect_uri(
        RedirectUrl::new(service.provider_callback.clone())
            .map_err(|error| ApiError::Internal(error.into()))?,
    );
    let (provider_challenge, provider_verifier) = PkceCodeChallenge::new_random_sha256();
    let (authorization_url, provider_state, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("email".to_owned()))
        .add_scope(Scope::new("profile".to_owned()))
        .set_pkce_challenge(provider_challenge)
        .url();
    let state_hash = auth::opaque_token_hash(provider_state.secret());
    let expires_at = Utc::now() + chrono::Duration::minutes(AUTHORIZATION_LIFETIME_MINUTES);
    sqlx::query(
        r#"
        INSERT INTO oidc_authorizations (
            id, state_hash, nonce, provider_pkce_verifier, device_id,
            client_code_challenge, client_state, client_redirect_uri,
            link_user_id, expires_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(state_hash.as_slice())
    .bind(nonce.secret())
    .bind(provider_verifier.secret())
    .bind(request.device_id)
    .bind(request.code_challenge)
    .bind(request.client_state)
    .bind(request.redirect_uri)
    .bind(link_user_id)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;
    Ok(Json(OidcAuthorization {
        schema_version: SCHEMA_VERSION,
        authorization_url: authorization_url.to_string(),
        expires_at,
    }))
}

pub async fn callback(
    State(state): State<AppState>,
    Query(query): Query<OidcCallbackQuery>,
) -> Result<Redirect, ApiError> {
    let service = state
        .oidc
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("OIDC is not configured".to_owned()))?;
    if query.error.is_some() {
        return Err(ApiError::Unauthorized(
            "the identity provider rejected the authorization".to_owned(),
        ));
    }
    let code = query
        .code
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::BadRequest("OIDC callback is missing code".to_owned()))?;
    let state_hash = auth::opaque_token_hash(&query.state);
    let row = sqlx::query(
        r#"
        UPDATE oidc_authorizations SET consumed_at = now()
        WHERE state_hash = $1 AND consumed_at IS NULL AND expires_at > now()
        RETURNING nonce, provider_pkce_verifier, device_id, client_code_challenge,
                  client_state, client_redirect_uri, link_user_id, expires_at
        "#,
    )
    .bind(state_hash.as_slice())
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| {
        ApiError::Unauthorized("OIDC state is invalid, expired, or already used".to_owned())
    })?;

    let client = CoreClient::from_provider_metadata(
        service.metadata.clone(),
        ClientId::new(service.client_id.clone()),
        Some(ClientSecret::new(service.client_secret.clone())),
    )
    .set_redirect_uri(
        RedirectUrl::new(service.provider_callback.clone())
            .map_err(|error| ApiError::Internal(error.into()))?,
    );
    let token_response = client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|error| {
            ApiError::Unauthorized(format!("OIDC code exchange is unavailable: {error}"))
        })?
        .set_pkce_verifier(PkceCodeVerifier::new(row.get("provider_pkce_verifier")))
        .request_async(&service.http)
        .await
        .map_err(|error| ApiError::Unauthorized(format!("OIDC code exchange failed: {error}")))?;
    let id_token = token_response.id_token().ok_or_else(|| {
        ApiError::Unauthorized("OIDC token response did not contain an ID token".to_owned())
    })?;
    let verifier = client.id_token_verifier();
    let nonce = Nonce::new(row.get("nonce"));
    let claims = id_token.claims(&verifier, &nonce).map_err(|error| {
        ApiError::Unauthorized(format!("OIDC ID token validation failed: {error}"))
    })?;
    if let Some(expected_hash) = claims.access_token_hash() {
        let actual_hash = AccessTokenHash::from_token(
            token_response.access_token(),
            id_token.signing_alg().map_err(|error| {
                ApiError::Unauthorized(format!("OIDC signing algorithm is invalid: {error}"))
            })?,
            id_token.signing_key(&verifier).map_err(|error| {
                ApiError::Unauthorized(format!("OIDC signing key is invalid: {error}"))
            })?,
        )
        .map_err(|error| {
            ApiError::Unauthorized(format!("OIDC access-token hash is invalid: {error}"))
        })?;
        if actual_hash != *expected_hash {
            return Err(ApiError::Unauthorized(
                "OIDC access-token hash validation failed".to_owned(),
            ));
        }
    }
    let subject = claims.subject().as_str().to_owned();
    let email = claims
        .email()
        .map(|value| value.as_str().trim().to_ascii_lowercase());
    let email_verified = claims.email_verified().unwrap_or(false);
    let link_user_id: Option<Uuid> = row.get("link_user_id");
    let user_id = if let Some(link_user_id) = link_user_id {
        link_identity(&state, service, link_user_id, &subject, email.as_deref()).await?
    } else {
        resolve_user(&state, service, &subject, email.as_deref(), email_verified).await?
    };

    let device_id: Uuid = row.get("device_id");
    let client_code_challenge: String = row.get("client_code_challenge");
    let mut tx = state.pool.begin().await?;
    let authorization = auth::create_native_authorization_code_tx(
        &mut tx,
        user_id,
        device_id,
        client_code_challenge,
    )
    .await?;
    sqlx::query("INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, $3, 'user', $2, $4)")
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(if link_user_id.is_some() { "auth.oidc.link" } else { "auth.oidc.login" })
        .bind(json!({"issuer": service.issuer, "subject_hash": hex::encode(Sha256::digest(subject.as_bytes()))}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let redirect_uri: String = row.get("client_redirect_uri");
    let mut redirect =
        Url::parse(&redirect_uri).map_err(|error| ApiError::Internal(error.into()))?;
    redirect
        .query_pairs_mut()
        .append_pair("code", &authorization.code)
        .append_pair("state", row.get::<String, _>("client_state").as_str());
    Ok(Redirect::to(redirect.as_str()))
}

async fn link_identity(
    state: &AppState,
    service: &OidcService,
    user_id: Uuid,
    subject: &str,
    email: Option<&str>,
) -> Result<Uuid, ApiError> {
    let mut tx = state.pool.begin().await?;
    let active_user = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    if !active_user {
        return Err(ApiError::Unauthorized(
            "the linking user no longer exists".to_owned(),
        ));
    }
    if let Some(owner) = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM oidc_identities WHERE issuer = $1 AND subject = $2 FOR UPDATE",
    )
    .bind(&service.issuer)
    .bind(subject)
    .fetch_optional(&mut *tx)
    .await?
    {
        if owner != user_id {
            return Err(ApiError::Conflict(
                "this OIDC identity is already linked to another account".to_owned(),
            ));
        }
        sqlx::query(
            "UPDATE oidc_identities SET last_login_at = now() WHERE issuer = $1 AND subject = $2",
        )
        .bind(&service.issuer)
        .bind(subject)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query("INSERT INTO oidc_identities (id, user_id, issuer, subject, email_at_link, last_login_at) VALUES ($1, $2, $3, $4, $5, now())")
            .bind(Uuid::new_v4())
            .bind(user_id)
            .bind(&service.issuer)
            .bind(subject)
            .bind(email)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(user_id)
}

async fn resolve_user(
    state: &AppState,
    service: &OidcService,
    subject: &str,
    email: Option<&str>,
    email_verified: bool,
) -> Result<Uuid, ApiError> {
    if let Some(user_id) = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE oidc_identities identity SET last_login_at = now()
        FROM users account
        WHERE identity.issuer = $1 AND identity.subject = $2
          AND account.id = identity.user_id AND account.deleted_at IS NULL
        RETURNING identity.user_id
        "#,
    )
    .bind(&service.issuer)
    .bind(subject)
    .fetch_optional(&state.pool)
    .await?
    {
        return Ok(user_id);
    }
    if !service.auto_provision {
        return Err(ApiError::Unauthorized(
            "this OIDC identity has not been provisioned for Open Cowork".to_owned(),
        ));
    }
    let email = email.filter(|_| email_verified).ok_or_else(|| {
        ApiError::Unauthorized("OIDC auto-provisioning requires a verified email claim".to_owned())
    })?;
    auth::validate_email_for_external_identity(email)?;
    let mut tx = state.pool.begin().await?;
    let user_id = if let Some(user_id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM users WHERE lower(email) = lower($1) AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(email)
    .fetch_optional(&mut *tx)
    .await?
    {
        user_id
    } else {
        let user_id = Uuid::new_v4();
        let display_name = email.split('@').next().unwrap_or("OIDC user");
        sqlx::query(
            "INSERT INTO users (id, etag, email, display_name, password_hash) VALUES ($1, $2, lower($3), $4, NULL)",
        )
        .bind(user_id)
        .bind(format!("W/\"{user_id}:1\""))
        .bind(email)
        .bind(display_name)
        .execute(&mut *tx)
        .await?;
        user_id
    };
    sqlx::query(
        "INSERT INTO oidc_identities (id, user_id, issuer, subject, email_at_link, last_login_at) VALUES ($1, $2, $3, $4, $5, now())",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(&service.issuer)
    .bind(subject)
    .bind(email)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(user_id)
}

fn validate_client_state(value: &str) -> Result<(), ApiError> {
    if !(43..=128).contains(&value.len())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ApiError::Unprocessable(
            "client_state must contain 43 to 128 base64url characters".to_owned(),
        ));
    }
    Ok(())
}

fn validated_https_or_loopback_url(value: &str, name: &str) -> anyhow::Result<Url> {
    let url = Url::parse(value)?;
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        anyhow::bail!("{name} must use HTTPS except on loopback");
    }
    if url.username() != "" || url.password().is_some() {
        anyhow::bail!("{name} cannot contain URL credentials");
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_and_state_validation_are_exact() {
        assert!(validate_client_state(&"a".repeat(43)).is_ok());
        assert!(validate_client_state("short").is_err());
        assert!(validated_https_or_loopback_url("https://id.example.test", "issuer").is_ok());
        assert!(validated_https_or_loopback_url("http://id.example.test", "issuer").is_err());
        assert!(validated_https_or_loopback_url("http://127.0.0.1:9999", "issuer").is_ok());
    }
}
