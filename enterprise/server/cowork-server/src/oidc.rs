use axum::{
    extract::{Path, Query, State},
    response::Redirect,
    Json,
};
use chrono::Utc;
use cowork_contracts::{
    OidcAuthorization, OidcConfiguration, StartOidcAuthorizationRequest, SCHEMA_VERSION,
};
use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
    AccessTokenHash, AuthenticationContextClass, AuthorizationCode, ClientId, ClientSecret,
    CsrfToken, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse,
};
use reqwest::{redirect::Policy, Client, Url};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
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
    pub(crate) auto_provision: bool,
    assurance: Option<crate::oidc_assurance::Policy>,
    http: Client,
}

impl OidcService {
    pub fn assurance_policy(&self) -> Option<&crate::oidc_assurance::Policy> {
        self.assurance.as_ref()
    }
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
    let assurance = config
        .assurance
        .clone()
        .map(|policy| policy.validate(&config.issuer, &config.client_id))
        .transpose()?;
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
    let http = Client::builder()
        .redirect(Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let configured_issuer = config.issuer.trim().to_owned();
    let issuer_url = IssuerUrl::new(configured_issuer.clone())?;
    let metadata = CoreProviderMetadata::discover_async(issuer_url, &http).await?;
    if metadata.issuer().as_str() != configured_issuer {
        anyhow::bail!("OIDC discovery issuer does not exactly match COWORK_OIDC_ISSUER");
    }
    validated_https_or_loopback_url(
        metadata.authorization_endpoint().as_str(),
        "OIDC authorization endpoint",
    )?;
    validated_https_or_loopback_url(metadata.jwks_uri().as_str(), "OIDC JWKS endpoint")?;
    if let Some(endpoint) = metadata.token_endpoint() {
        validated_https_or_loopback_url(endpoint.as_str(), "OIDC token endpoint")?;
    }
    Ok(Some(OidcService {
        metadata,
        client_id: config.client_id.clone(),
        client_secret: config.client_secret.clone(),
        issuer: configured_issuer,
        provider_callback: format!("{public_origin}/api/v1/auth/oidc/callback"),
        web_callback: format!("{public_origin}/auth/callback"),
        auto_provision: config.auto_provision,
        assurance,
        http,
    }))
}

pub async fn configuration(State(state): State<AppState>) -> Json<OidcConfiguration> {
    Json(OidcConfiguration {
        schema_version: SCHEMA_VERSION,
        enabled: !state.management_mode && state.oidc.is_some(),
    })
}

fn require_login_scope(management_mode: bool, organisation: Option<Uuid>) -> Result<(), ApiError> {
    // Management SSO is admitted by an organisation administrator through the
    // provider catalog. Legacy operator environment settings belong to workspace
    // servers and must not provision an account ahead of the first local admin.
    if management_mode && organisation.is_none() {
        return Err(ApiError::NotFound("OIDC is not configured".into()));
    }
    Ok(())
}

pub async fn start(
    State(state): State<AppState>,
    Json(request): Json<StartOidcAuthorizationRequest>,
) -> Result<Json<OidcAuthorization>, ApiError> {
    start_authorization(&state, request, None, None).await
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
    start_authorization(
        &state,
        request,
        Some((principal.user_id, principal.session_id.unwrap())),
        None,
    )
    .await
}

pub async fn start_organisation(
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Json(request): Json<StartOidcAuthorizationRequest>,
) -> Result<Json<OidcAuthorization>, ApiError> {
    start_authorization(&state, request, None, Some(org)).await
}
pub async fn link_organisation(
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    axum::extract::Extension(principal): axum::extract::Extension<Principal>,
    Json(request): Json<StartOidcAuthorizationRequest>,
) -> Result<Json<OidcAuthorization>, ApiError> {
    let session = principal
        .session_id
        .ok_or_else(|| ApiError::Unauthorized("linking requires a user session".into()))?;
    start_authorization(
        &state,
        request,
        Some((principal.user_id, session)),
        Some(org),
    )
    .await
}

async fn start_authorization(
    state: &AppState,
    request: StartOidcAuthorizationRequest,
    link: Option<(Uuid, Uuid)>,
    organisation: Option<Uuid>,
) -> Result<Json<OidcAuthorization>, ApiError> {
    require_login_scope(state.management_mode, organisation)?;
    let context = match organisation {
        Some(org) => Some(crate::management::sso::context(state, org).await?),
        None => None,
    };
    let service = match &context {
        Some(ctx) => {
            ctx.provider
                .service(ctx.configuration.auto_provision)
                .await?
        }
        None => state
            .oidc
            .clone()
            .ok_or_else(|| ApiError::NotFound("OIDC is not configured".into()))?,
    };
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
    let mut authorization = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("email".to_owned()))
        .add_scope(Scope::new("profile".to_owned()))
        .set_pkce_challenge(provider_challenge);
    if let Some(policy) = service.assurance_policy() {
        authorization = authorization.set_max_age(std::time::Duration::from_secs(
            policy.max_age_seconds.into(),
        ));
        let mut contexts: Vec<_> = policy
            .mappings
            .iter()
            .filter_map(|m| m.acr.as_ref())
            .collect();
        contexts.sort();
        contexts.dedup();
        for context in contexts {
            authorization = authorization
                .add_auth_context_value(AuthenticationContextClass::new(context.clone()));
        }
    }
    if let Some(ctx) = &context {
        for scope in &ctx.provider.scopes {
            authorization = authorization.add_scope(Scope::new(scope.clone()));
        }
    }
    let (authorization_url, provider_state, nonce) = authorization.url();
    let state_hash = auth::opaque_token_hash(provider_state.secret());
    let expires_at = Utc::now() + chrono::Duration::minutes(AUTHORIZATION_LIFETIME_MINUTES);
    sqlx::query(
        r#"
        INSERT INTO oidc_authorizations (
            id, state_hash, nonce, provider_pkce_verifier, device_id,
            client_code_challenge, client_state, client_redirect_uri,
            link_user_id, expires_at, link_session_id, organisation_id,organisation_sso_revision,organisation_provider_fingerprint
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,$12,$13,$14)
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
    .bind(link.map(|(user, _)| user))
    .bind(expires_at)
    .bind(link.map(|(_, session)| session))
    .bind(organisation)
    .bind(context.as_ref().map(|ctx|ctx.configuration.revision))
    .bind(context.as_ref().map(|ctx|&ctx.provider.fingerprint))
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
    if !(22..=128).contains(&query.state.len())
        || !query
            .state
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(ApiError::Unauthorized("OIDC state is invalid".into()));
    }
    let state_hash = auth::opaque_token_hash(&query.state);
    let row = sqlx::query(
        r#"
        UPDATE oidc_authorizations SET consumed_at = now()
        WHERE state_hash = $1 AND consumed_at IS NULL AND expires_at > now()
        RETURNING nonce, provider_pkce_verifier, device_id, client_code_challenge,
                  client_state, client_redirect_uri, link_user_id, link_session_id, expires_at,
                  organisation_id,organisation_sso_revision,organisation_provider_fingerprint
        "#,
    )
    .bind(state_hash.as_slice())
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| {
        ApiError::Unauthorized("OIDC state is invalid, expired, or already used".to_owned())
    })?;
    let organisation = row.get::<Option<Uuid>, _>("organisation_id");
    // Also reject global authorizations issued before this replica was upgraded.
    // Consuming their state above prevents a delayed callback from provisioning.
    require_login_scope(state.management_mode, organisation)?;
    let context = match organisation {
        Some(org) => {
            let ctx = crate::management::sso::context(&state, org).await?;
            if Some(ctx.configuration.revision) != row.get("organisation_sso_revision")
                || Some(&ctx.provider.fingerprint)
                    != row
                        .get::<Option<String>, _>("organisation_provider_fingerprint")
                        .as_ref()
            {
                return Err(ApiError::Unauthorized(
                    "organisation SSO changed; start sign-in again".into(),
                ));
            }
            Some(ctx)
        }
        None => None,
    };
    let service = match &context {
        Some(ctx) => {
            ctx.provider
                .service(ctx.configuration.auto_provision)
                .await?
        }
        None => state
            .oidc
            .clone()
            .ok_or_else(|| ApiError::NotFound("OIDC is not configured".into()))?,
    };
    // A denied or malformed provider response also consumes this authorization.
    if query.error.is_some() {
        return Err(ApiError::Unauthorized(
            "the identity provider rejected the authorization".into(),
        ));
    }
    let code = query
        .code
        .filter(|value| !value.trim().is_empty() && value.len() <= 8192)
        .ok_or_else(|| ApiError::BadRequest("OIDC callback is missing a valid code".into()))?;

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
        .map_err(|_| ApiError::Unauthorized("OIDC code exchange is unavailable".into()))?
        .set_pkce_verifier(PkceCodeVerifier::new(row.get("provider_pkce_verifier")))
        .request_async(&service.http)
        .await
        .map_err(|_| ApiError::Unauthorized("OIDC code exchange failed".into()))?;
    let id_token = token_response.id_token().ok_or_else(|| {
        ApiError::Unauthorized("OIDC token response did not contain an ID token".to_owned())
    })?;
    let verifier = client.id_token_verifier();
    let nonce = Nonce::new(row.get("nonce"));
    let claims = id_token
        .claims(&verifier, &nonce)
        .map_err(|_| ApiError::Unauthorized("OIDC ID token validation failed".into()))?;
    // openidconnect validates aud but deliberately leaves azp policy to callers.
    if claims
        .authorized_party()
        .is_some_and(|party| party.as_str() != service.client_id)
        || (claims.audiences().len() > 1 && claims.authorized_party().is_none())
    {
        return Err(ApiError::Unauthorized(
            "OIDC authorized party is invalid".into(),
        ));
    }
    if let Some(expected_hash) = claims.access_token_hash() {
        let actual_hash = AccessTokenHash::from_token(
            token_response.access_token(),
            id_token
                .signing_alg()
                .map_err(|_| ApiError::Unauthorized("OIDC signing algorithm is invalid".into()))?,
            id_token
                .signing_key(&verifier)
                .map_err(|_| ApiError::Unauthorized("OIDC signing key is invalid".into()))?,
        )
        .map_err(|_| ApiError::Unauthorized("OIDC access-token hash is invalid".into()))?;
        if actual_hash != *expected_hash {
            return Err(ApiError::Unauthorized(
                "OIDC access-token hash validation failed".to_owned(),
            ));
        }
    }
    let subject = claims.subject().as_str().to_owned();
    if subject.is_empty() || subject.len() > 255 || !subject.is_ascii() {
        return Err(ApiError::Unauthorized("OIDC subject is invalid".into()));
    }
    let email = claims
        .email()
        .map(|value| value.as_str().trim().to_ascii_lowercase());
    let email_verified = claims.email_verified().unwrap_or(false);
    let link_user_id: Option<Uuid> = row.get("link_user_id");
    let mut tx = state.pool.begin().await?;
    let group_role = context
        .as_ref()
        .and_then(|ctx| crate::management::sso::verified_group_role(&id_token.to_string(), ctx));
    if let Some(ctx) = &context {
        crate::management::sso::recheck(&mut tx, ctx).await?;
        lock_identity(&mut tx, &service.issuer, &subject).await?;
        let known:Option<Uuid>=sqlx::query_scalar("SELECT i.user_id FROM oidc_identities i JOIN users u ON u.id=i.user_id WHERE i.issuer=$1 AND i.subject=$2 AND i.binding_method<>'legacy_unverified' AND u.deleted_at IS NULL")
            .bind(&service.issuer).bind(&subject).fetch_optional(&mut *tx).await?;
        // Denied linking must not mutate membership after logout or MFA expiry.
        if let Some(user) = link_user_id {
            let session = row
                .get::<Option<Uuid>, _>("link_session_id")
                .ok_or_else(|| {
                    ApiError::Unauthorized("start linking again from an active session".into())
                })?;
            validate_link_session(&state, &mut tx, user, session).await?;
        }
        if !crate::management::sso::admit(&mut tx, ctx, known.or(link_user_id), group_role).await? {
            tx.commit().await?;
            return Err(ApiError::Unauthorized(
                "no organisation membership matches the signed identity claims".into(),
            ));
        }
    }
    let user_id = if let Some(link_user_id) = link_user_id {
        let session = row
            .get::<Option<Uuid>, _>("link_session_id")
            .ok_or_else(|| {
                ApiError::Unauthorized("start account linking again from an active session".into())
            })?;
        link_identity(
            &state,
            &mut tx,
            &service,
            link_user_id,
            session,
            &subject,
            email.as_deref(),
        )
        .await?
    } else {
        resolve_user(
            &mut tx,
            &service,
            &subject,
            email.as_deref(),
            email_verified,
        )
        .await?
    };
    if let Some(ctx) = &context {
        crate::management::sso::sync_member(&mut tx, ctx, user_id, group_role).await?;
    }
    let origin = crate::management::directory::oidc_origin(
        &state,
        &mut tx,
        user_id,
        &service.issuer,
        &service.client_id,
        &subject,
        context.as_ref(),
    )
    .await?;

    let device_id: Uuid = row.get("device_id");
    let client_code_challenge: String = row.get("client_code_challenge");
    let provider_evidence = crate::oidc_assurance::from_verified_claims(
        &mut tx,
        &service,
        context.as_ref(),
        user_id,
        claims,
    )
    .await?;
    let authorization = auth::create_native_authorization_code_tx(
        &mut tx,
        user_id,
        device_id,
        client_code_challenge,
        None, // Local factor evidence is never inferred from provider claims.
    )
    .await?;
    crate::management::directory::stamp_code(&mut tx, &authorization.code, &origin).await?;
    if let Some(proof) = &provider_evidence {
        crate::oidc_assurance::stamp_native(&mut tx, &authorization.code, proof).await?;
    }
    sqlx::query("INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, $3, 'user', $2, $4)")
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(if link_user_id.is_some() { "auth.oidc.link" } else { "auth.oidc.login" })
        .bind(json!({"issuer": service.issuer, "subject_hash": hex::encode(Sha256::digest(subject.as_bytes())), "provider_assurance":provider_evidence}))
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

async fn validate_link_session(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<(), ApiError> {
    // Hold both rows through the binding write. Logout, deletion and factor
    // removal cannot race a callback that has only checked an old snapshot.
    let session = sqlx::query("SELECT u.platform_admin,s.oidc_assurance, coalesce(s.second_factor_verified_at>now()-interval '12 hours' AND s.second_factor_verified_at<=now(),false) AND (EXISTS(SELECT 1 FROM user_totp t WHERE t.user_id=u.id AND t.enabled) OR EXISTS(SELECT 1 FROM passkeys p WHERE p.user_id=u.id)) AS verified FROM users u JOIN auth_sessions s ON s.user_id=u.id WHERE u.id=$1 AND s.id=$2 AND u.deleted_at IS NULL AND s.revoked_at IS NULL AND s.expires_at>now() FOR SHARE OF u,s")
        .bind(user_id).bind(session_id).fetch_optional(&mut **tx).await?
        .ok_or_else(|| ApiError::Unauthorized("account linking requires the original active session; sign in and start again".into()))?;
    let mut verified = session.get::<bool, _>("verified");
    if !verified {
        if let Some(proof) = crate::oidc_assurance::evidence(session.get("oidc_assurance")) {
            verified = crate::oidc_assurance::valid_in_tx(state, tx, user_id, &proof).await?;
        }
    }
    if state.management_mode && session.get::<bool, _>("platform_admin") && !verified {
        return Err(ApiError::MfaRequired(
            "verify the administrator session and start account linking again".into(),
        ));
    }
    Ok(())
}

async fn link_identity(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    service: &OidcService,
    user_id: Uuid,
    session_id: Uuid,
    subject: &str,
    email: Option<&str>,
) -> Result<Uuid, ApiError> {
    lock_identity(tx, &service.issuer, subject).await?;
    validate_link_session(state, tx, user_id, session_id).await?;
    if let Some(owner) = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM oidc_identities WHERE issuer = $1 AND subject = $2 FOR UPDATE",
    )
    .bind(&service.issuer)
    .bind(subject)
    .fetch_optional(&mut **tx)
    .await?
    {
        if owner != user_id {
            return Err(ApiError::Conflict(
                "this OIDC identity is already linked to another account".to_owned(),
            ));
        }
        sqlx::query(
            "UPDATE oidc_identities SET last_login_at = now(),binding_method='authenticated_session' WHERE issuer = $1 AND subject = $2",
        )
        .bind(&service.issuer)
        .bind(subject)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query("INSERT INTO oidc_identities (id, user_id, issuer, subject, email_at_link, last_login_at,binding_method) VALUES ($1, $2, $3, $4, $5, now(),'authenticated_session')")
            .bind(Uuid::new_v4())
            .bind(user_id)
            .bind(&service.issuer)
            .bind(subject)
            .bind(email)
            .execute(&mut **tx)
            .await?;
    }
    Ok(user_id)
}

async fn resolve_user(
    tx: &mut Transaction<'_, Postgres>,
    service: &OidcService,
    subject: &str,
    email: Option<&str>,
    email_verified: bool,
) -> Result<Uuid, ApiError> {
    lock_identity(tx, &service.issuer, subject).await?;
    if let Some(identity) = sqlx::query("SELECT i.user_id,i.binding_method,u.deleted_at FROM oidc_identities i JOIN users u ON u.id=i.user_id WHERE i.issuer=$1 AND i.subject=$2 FOR UPDATE OF i FOR SHARE OF u")
        .bind(&service.issuer).bind(subject).fetch_optional(&mut **tx).await? {
        if identity.get::<Option<chrono::DateTime<Utc>>,_>("deleted_at").is_some() {
            return Err(ApiError::Unauthorized("this OIDC identity is unavailable".into()));
        }
        if identity.get::<String,_>("binding_method")=="legacy_unverified" {
            return Err(binding_required());
        }
        let user_id:Uuid=identity.get("user_id");
        sqlx::query("UPDATE oidc_identities SET last_login_at=now() WHERE issuer=$1 AND subject=$2")
            .bind(&service.issuer).bind(subject).execute(&mut **tx).await?;
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
    let user_id = Uuid::new_v4();
    let created:Option<Uuid>=sqlx::query_scalar(
        "INSERT INTO users (id,etag,email,display_name,password_hash) VALUES($1,$2,lower($3),$4,NULL) ON CONFLICT (lower(email)) WHERE deleted_at IS NULL DO NOTHING RETURNING id"
    ).bind(user_id).bind(format!("W/\"{user_id}:1\""))
        .bind(email).bind(email.split('@').next().unwrap_or("OIDC user"))
        .fetch_optional(&mut **tx).await?;
    if created.is_none() {
        return Err(binding_required());
    }
    sqlx::query(
        "INSERT INTO oidc_identities (id, user_id, issuer, subject, email_at_link, last_login_at,binding_method) VALUES ($1, $2, $3, $4, $5, now(),'provisioned')",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(&service.issuer)
    .bind(subject)
    .bind(email)
    .execute(&mut **tx)
    .await?;
    Ok(user_id)
}

fn binding_required() -> ApiError {
    ApiError::Conflict("sign into your existing account first, then link SSO from Security; an email match does not link accounts".into())
}

async fn lock_identity(
    tx: &mut Transaction<'_, Postgres>,
    issuer: &str,
    subject: &str,
) -> Result<(), ApiError> {
    // Serialize first-time provisioning and explicit linking for this exact
    // external identity. A hash collision only reduces concurrency.
    let hash = Sha256::digest(serde_json::to_vec(&("oidc-identity", issuer, subject))?);
    let key = i64::from_be_bytes(hash[..8].try_into().expect("SHA256 has eight bytes"));
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut **tx)
        .await?;
    Ok(())
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

pub(crate) fn validated_https_or_loopback_url(value: &str, name: &str) -> anyhow::Result<Url> {
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
    fn management_login_requires_an_organisation_but_workspace_legacy_remains_available() {
        assert!(matches!(
            require_login_scope(true, None),
            Err(ApiError::NotFound(_))
        ));
        assert!(require_login_scope(true, Some(Uuid::new_v4())).is_ok());
        assert!(require_login_scope(false, None).is_ok());
    }

    #[test]
    fn callback_and_state_validation_are_exact() {
        assert!(validate_client_state(&"a".repeat(43)).is_ok());
        assert!(validate_client_state("short").is_err());
        assert!(validated_https_or_loopback_url("https://id.example.test", "issuer").is_ok());
        assert!(validated_https_or_loopback_url("http://id.example.test", "issuer").is_err());
        assert!(validated_https_or_loopback_url("http://127.0.0.1:9999", "issuer").is_ok());
    }
}
