use axum::{
    extract::{Extension, Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use cowork_contracts::{
    AuthTokens, DeletePasskeyRequest, FinishNativePasskeyAuthenticationRequest,
    FinishPasskeyAuthenticationRequest, FinishPasskeyRegistrationRequest,
    NativePasskeyAuthorizationResult, NativePasskeyChallenge, PasskeyChallenge, PasskeyRecord,
    StartNativePasskeyAuthenticationRequest, StartPasskeyAuthenticationRequest, SCHEMA_VERSION,
};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use webauthn_rs::prelude::{
    AuthenticationResult, CredentialID, Passkey, PasskeyAuthentication, PasskeyRegistration,
    PublicKeyCredential, RegisterPublicKeyCredential, Url, Webauthn, WebauthnBuilder,
};

use crate::{
    auth::{self, Principal},
    config::PasskeyConfig,
    error::ApiError,
    AppState,
};

const CHALLENGE_MINUTES: i64 = 5;
const NATIVE_REDIRECT_URI: &str = "open-cowork://auth/callback";

pub fn build_webauthn(config: &PasskeyConfig) -> anyhow::Result<Webauthn> {
    let origin = Url::parse(&config.origin)?;
    Ok(WebauthnBuilder::new(&config.rp_id, &origin)?
        .rp_name("Open Cowork")
        .build()?)
}

pub async fn native_authorization_page() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        NATIVE_AUTHORIZATION_HTML,
    )
}

pub async fn native_authorization_script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        NATIVE_AUTHORIZATION_JS,
    )
}

pub async fn start_native_authentication(
    State(state): State<AppState>,
    Json(request): Json<StartNativePasskeyAuthenticationRequest>,
) -> Result<Json<NativePasskeyChallenge>, ApiError> {
    let webauthn = configured(&state)?;
    auth::validate_pkce_challenge(&request.code_challenge, &request.code_challenge_method)?;
    validate_native_state(&request.state)?;
    if request.redirect_uri != NATIVE_REDIRECT_URI {
        return Err(ApiError::Unprocessable(
            "native passkey redirect_uri is not allowed".to_owned(),
        ));
    }
    let user_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM users WHERE lower(email) = lower($1) AND deleted_at IS NULL",
    )
    .bind(request.email.trim())
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(invalid_passkey_login)?;
    let rows = sqlx::query("SELECT credential FROM passkeys WHERE user_id = $1")
        .bind(user_id)
        .fetch_all(&state.pool)
        .await?;
    let credentials = rows
        .iter()
        .map(|row| serde_json::from_value::<Passkey>(row.get("credential")))
        .collect::<Result<Vec<_>, _>>()
        .map_err(ApiError::Json)?;
    if credentials.is_empty() {
        return Err(invalid_passkey_login());
    }
    let (challenge, authentication) = webauthn
        .start_passkey_authentication(&credentials)
        .map_err(passkey_error)?;
    let authorization_id = Uuid::new_v4();
    let challenge_id = Uuid::new_v4();
    let expires_at = Utc::now() + chrono::Duration::minutes(CHALLENGE_MINUTES);
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO native_passkey_authorizations (id, user_id, device_id, code_challenge, client_state, redirect_uri, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7)")
        .bind(authorization_id).bind(user_id).bind(request.device_id)
        .bind(request.code_challenge).bind(request.state).bind(request.redirect_uri)
        .bind(expires_at).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO webauthn_challenges (id, kind, user_id, device_id, state, authorization_id, expires_at) VALUES ($1, 'native_authentication', $2, $3, $4, $5, $6)")
        .bind(challenge_id).bind(user_id).bind(request.device_id)
        .bind(serde_json::to_value(authentication)?).bind(authorization_id)
        .bind(expires_at).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(NativePasskeyChallenge {
        schema_version: SCHEMA_VERSION,
        challenge_id,
        authorization_id,
        public_key: serde_json::to_value(challenge)?,
        expires_at,
    }))
}

pub async fn finish_native_authentication(
    State(state): State<AppState>,
    Json(request): Json<FinishNativePasskeyAuthenticationRequest>,
) -> Result<Json<NativePasskeyAuthorizationResult>, ApiError> {
    let webauthn = configured(&state)?;
    let mut tx = state.pool.begin().await?;
    let challenge = sqlx::query(
        "UPDATE webauthn_challenges SET used_at = now() WHERE id = $1 AND kind = 'native_authentication' AND authorization_id = $2 AND used_at IS NULL AND expires_at > now() RETURNING user_id, device_id, state",
    )
    .bind(request.challenge_id)
    .bind(request.authorization_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(invalid_passkey_login)?;
    let authorization = sqlx::query(
        "SELECT user_id, device_id, code_challenge, client_state, redirect_uri FROM native_passkey_authorizations WHERE id = $1 AND consumed_at IS NULL AND expires_at > now() FOR UPDATE",
    )
    .bind(request.authorization_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(invalid_passkey_login)?;
    let user_id: Uuid = challenge.get("user_id");
    let device_id: Uuid = challenge
        .try_get::<Option<Uuid>, _>("device_id")?
        .ok_or_else(invalid_passkey_login)?;
    if authorization.get::<Uuid, _>("user_id") != user_id
        || authorization.get::<Uuid, _>("device_id") != device_id
    {
        return Err(invalid_passkey_login());
    }
    let authentication: PasskeyAuthentication = serde_json::from_value(challenge.get("state"))?;
    let credential: PublicKeyCredential =
        serde_json::from_value(request.credential).map_err(|_| invalid_passkey_login())?;
    let result = webauthn
        .finish_passkey_authentication(&credential, &authentication)
        .map_err(|_| invalid_passkey_login())?;
    let passkey_id = update_matching_passkey(&mut tx, user_id, &result).await?;
    sqlx::query("UPDATE native_passkey_authorizations SET consumed_at = now() WHERE id = $1")
        .bind(request.authorization_id)
        .execute(&mut *tx)
        .await?;
    let native_code = auth::create_native_authorization_code_tx(
        &mut tx,
        user_id,
        device_id,
        authorization.get("code_challenge"),
    )
    .await?;
    audit(
        &mut tx,
        user_id,
        "auth.passkey.native_authorized",
        passkey_id,
    )
    .await?;
    let mut redirect = Url::parse(authorization.get::<String, _>("redirect_uri").as_str())
        .map_err(|error| ApiError::Internal(error.into()))?;
    redirect
        .query_pairs_mut()
        .append_pair("code", &native_code.code)
        .append_pair(
            "state",
            authorization.get::<String, _>("client_state").as_str(),
        );
    tx.commit().await?;
    Ok(Json(NativePasskeyAuthorizationResult {
        schema_version: SCHEMA_VERSION,
        redirect_url: redirect.to_string(),
        expires_at: native_code.expires_at,
    }))
}

pub async fn start_registration(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<PasskeyChallenge>, ApiError> {
    let webauthn = configured(&state)?;
    let user =
        sqlx::query("SELECT email, display_name FROM users WHERE id = $1 AND deleted_at IS NULL")
            .bind(principal.user_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("user account is unavailable".to_owned()))?;
    let credential_rows = sqlx::query("SELECT credential_id FROM passkeys WHERE user_id = $1")
        .bind(principal.user_id)
        .fetch_all(&state.pool)
        .await?;
    let excluded = credential_rows
        .iter()
        .map(|row| CredentialID::from(row.get::<Vec<u8>, _>("credential_id")))
        .collect::<Vec<_>>();
    let email: String = user.get("email");
    let display_name: String = user.get("display_name");
    let (challenge, registration) = webauthn
        .start_passkey_registration(
            principal.user_id,
            &email,
            &display_name,
            (!excluded.is_empty()).then_some(excluded),
        )
        .map_err(passkey_error)?;
    persist_challenge(
        &state,
        "registration",
        principal.user_id,
        None,
        serde_json::to_value(registration)?,
        serde_json::to_value(challenge)?,
    )
    .await
}

pub async fn finish_registration(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<FinishPasskeyRegistrationRequest>,
) -> Result<(StatusCode, Json<PasskeyRecord>), ApiError> {
    let webauthn = configured(&state)?;
    let label = request.label.trim();
    if label.is_empty() || label.chars().count() > 100 {
        return Err(ApiError::Unprocessable(
            "passkey label must contain 1 to 100 characters".to_owned(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        "UPDATE webauthn_challenges SET used_at = now() WHERE id = $1 AND kind = 'registration' AND user_id = $2 AND used_at IS NULL AND expires_at > now() RETURNING state",
    )
    .bind(request.challenge_id)
    .bind(principal.user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::Unauthorized("passkey registration challenge is invalid or expired".to_owned()))?;
    let registration: PasskeyRegistration = serde_json::from_value(row.get("state"))?;
    let credential: RegisterPublicKeyCredential = serde_json::from_value(request.credential)
        .map_err(|error| {
            ApiError::BadRequest(format!("invalid passkey registration credential: {error}"))
        })?;
    let passkey = webauthn
        .finish_passkey_registration(&credential, &registration)
        .map_err(passkey_error)?;
    let credential_id = passkey.cred_id().as_ref().to_vec();
    let duplicate = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM passkeys WHERE credential_id = $1)",
    )
    .bind(&credential_id)
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(ApiError::Conflict(
            "this passkey is already registered".to_owned(),
        ));
    }
    let id = Uuid::new_v4();
    let created_at = Utc::now();
    sqlx::query("INSERT INTO passkeys (id, user_id, credential_id, label, credential, created_at) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(id).bind(principal.user_id).bind(credential_id).bind(label)
        .bind(serde_json::to_value(passkey)?).bind(created_at).execute(&mut *tx).await?;
    audit(&mut tx, principal.user_id, "auth.passkey.registered", id).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(PasskeyRecord {
            schema_version: SCHEMA_VERSION,
            id,
            label: label.to_owned(),
            created_at,
            last_used_at: None,
        }),
    ))
}

pub async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<PasskeyRecord>>, ApiError> {
    let rows = sqlx::query("SELECT id, label, created_at, last_used_at FROM passkeys WHERE user_id = $1 ORDER BY created_at")
        .bind(principal.user_id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.iter()
            .map(|row| PasskeyRecord {
                schema_version: SCHEMA_VERSION,
                id: row.get("id"),
                label: row.get("label"),
                created_at: row.get("created_at"),
                last_used_at: row.get("last_used_at"),
            })
            .collect(),
    ))
}

pub async fn remove(
    State(state): State<AppState>,
    Path(passkey_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<DeletePasskeyRequest>,
) -> Result<StatusCode, ApiError> {
    auth::verify_account_password_and_second_factor(
        &state,
        principal.user_id,
        request.password,
        request.second_factor.as_deref(),
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    let deleted = sqlx::query("DELETE FROM passkeys WHERE id = $1 AND user_id = $2")
        .bind(passkey_id)
        .bind(principal.user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(ApiError::NotFound(format!(
            "passkey {passkey_id} was not found"
        )));
    }
    audit(
        &mut tx,
        principal.user_id,
        "auth.passkey.removed",
        passkey_id,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn start_authentication(
    State(state): State<AppState>,
    Json(request): Json<StartPasskeyAuthenticationRequest>,
) -> Result<Json<PasskeyChallenge>, ApiError> {
    let webauthn = configured(&state)?;
    let user_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM users WHERE lower(email) = lower($1) AND deleted_at IS NULL",
    )
    .bind(request.email.trim())
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(invalid_passkey_login)?;
    let rows = sqlx::query("SELECT credential FROM passkeys WHERE user_id = $1")
        .bind(user_id)
        .fetch_all(&state.pool)
        .await?;
    let credentials = rows
        .iter()
        .map(|row| serde_json::from_value::<Passkey>(row.get("credential")))
        .collect::<Result<Vec<_>, _>>()
        .map_err(ApiError::Json)?;
    if credentials.is_empty() {
        return Err(invalid_passkey_login());
    }
    let (challenge, authentication) = webauthn
        .start_passkey_authentication(&credentials)
        .map_err(passkey_error)?;
    persist_challenge(
        &state,
        "authentication",
        user_id,
        Some(request.device_id),
        serde_json::to_value(authentication)?,
        serde_json::to_value(challenge)?,
    )
    .await
}

pub async fn finish_authentication(
    State(state): State<AppState>,
    Json(request): Json<FinishPasskeyAuthenticationRequest>,
) -> Result<Json<AuthTokens>, ApiError> {
    let webauthn = configured(&state)?;
    let mut tx = state.pool.begin().await?;
    let challenge = sqlx::query(
        "UPDATE webauthn_challenges SET used_at = now() WHERE id = $1 AND kind = 'authentication' AND used_at IS NULL AND expires_at > now() RETURNING user_id, device_id, state",
    )
    .bind(request.challenge_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(invalid_passkey_login)?;
    let user_id: Uuid = challenge.get("user_id");
    let device_id: Uuid = challenge
        .try_get::<Option<Uuid>, _>("device_id")?
        .ok_or_else(|| {
            ApiError::Internal(anyhow::anyhow!("passkey challenge has no device binding"))
        })?;
    let authentication: PasskeyAuthentication = serde_json::from_value(challenge.get("state"))?;
    let credential: PublicKeyCredential =
        serde_json::from_value(request.credential).map_err(|_| invalid_passkey_login())?;
    let result = webauthn
        .finish_passkey_authentication(&credential, &authentication)
        .map_err(|_| invalid_passkey_login())?;
    let passkey_id = update_matching_passkey(&mut tx, user_id, &result).await?;
    let tokens = auth::create_session_tx(&mut tx, user_id, device_id).await?;
    audit(&mut tx, user_id, "auth.passkey.login", passkey_id).await?;
    tx.commit().await?;
    Ok(Json(tokens))
}

async fn update_matching_passkey(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    result: &AuthenticationResult,
) -> Result<Uuid, ApiError> {
    let rows = sqlx::query("SELECT id, credential FROM passkeys WHERE user_id = $1 FOR UPDATE")
        .bind(user_id)
        .fetch_all(&mut **tx)
        .await?;
    for row in rows {
        let mut passkey: Passkey = serde_json::from_value(row.get("credential"))?;
        if passkey.update_credential(result).is_some() {
            let id: Uuid = row.get("id");
            sqlx::query("UPDATE passkeys SET credential = $2, last_used_at = now() WHERE id = $1")
                .bind(id)
                .bind(serde_json::to_value(passkey)?)
                .execute(&mut **tx)
                .await?;
            return Ok(id);
        }
    }
    Err(invalid_passkey_login())
}

fn validate_native_state(state: &str) -> Result<(), ApiError> {
    if !(43..=128).contains(&state.len())
        || !state
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(ApiError::Unprocessable(
            "native authorization state must be 43 to 128 base64url characters".to_owned(),
        ));
    }
    Ok(())
}

async fn persist_challenge(
    state: &AppState,
    kind: &str,
    user_id: Uuid,
    device_id: Option<Uuid>,
    challenge_state: serde_json::Value,
    public_key: serde_json::Value,
) -> Result<Json<PasskeyChallenge>, ApiError> {
    let challenge_id = Uuid::new_v4();
    let expires_at = Utc::now() + chrono::Duration::minutes(CHALLENGE_MINUTES);
    sqlx::query("INSERT INTO webauthn_challenges (id, kind, user_id, device_id, state, expires_at) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(challenge_id).bind(kind).bind(user_id).bind(device_id).bind(challenge_state).bind(expires_at)
        .execute(&state.pool).await?;
    Ok(Json(PasskeyChallenge {
        schema_version: SCHEMA_VERSION,
        challenge_id,
        public_key,
        expires_at,
    }))
}

fn configured(state: &AppState) -> Result<&Webauthn, ApiError> {
    state.webauthn.as_deref().ok_or_else(|| {
        ApiError::Conflict("passkeys are not configured for this server domain".to_owned())
    })
}

fn passkey_error(error: webauthn_rs::prelude::WebauthnError) -> ApiError {
    tracing::warn!(?error, "WebAuthn validation failed");
    ApiError::Unauthorized("passkey ceremony failed validation".to_owned())
}

fn invalid_passkey_login() -> ApiError {
    ApiError::Unauthorized("passkey authentication is invalid or unavailable".to_owned())
}

async fn audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    action: &str,
    target_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, $3, 'passkey', $4, $5)")
        .bind(Uuid::new_v4()).bind(user_id).bind(action).bind(target_id).bind(json!({})).execute(&mut **tx).await?;
    Ok(())
}

const NATIVE_AUTHORIZATION_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>Sign in to Open Cowork</title>
  <style>
    :root { color-scheme: light dark; font-family: system-ui, sans-serif; }
    body { min-height: 100vh; margin: 0; display: grid; place-items: center; background: #0b111b; color: #e8eef7; }
    main { width: min(420px, calc(100vw - 36px)); display: grid; gap: 16px; padding: 28px; border: 1px solid #263449; border-radius: 18px; background: #111b29; box-sizing: border-box; }
    h1, p { margin: 0; } p, small { color: #a9b6c8; } button { min-height: 48px; border: 0; border-radius: 10px; background: #3dd6b5; color: #06130f; font-weight: 750; cursor: pointer; }
    button:disabled { opacity: .6; cursor: wait; } #error { color: #ff9ca8; overflow-wrap: anywhere; } code { overflow-wrap: anywhere; }
  </style>
</head>
<body>
  <main>
    <small>Open Cowork</small>
    <h1>Confirm passkey sign-in</h1>
    <p>The passkey remains in your browser or password manager. The native app receives only a short-lived, PKCE-bound authorization code.</p>
    <code id="account"></code>
    <button id="authorize" type="button">Continue with passkey</button>
    <p id="status" role="status"></p>
    <p id="error" role="alert"></p>
  </main>
  <script src="/api/v1/auth/native/passkey/client.js"></script>
</body>
</html>"#;

const NATIVE_AUTHORIZATION_JS: &str = r#"(() => {
  'use strict';
  const params = new URLSearchParams(location.hash.slice(1));
  const required = ['email', 'device_id', 'code_challenge', 'state'];
  const missing = required.filter((name) => !params.get(name));
  const button = document.getElementById('authorize');
  const status = document.getElementById('status');
  const error = document.getElementById('error');
  document.getElementById('account').textContent = params.get('email') || '';
  if (missing.length || !window.PublicKeyCredential || !navigator.credentials) {
    error.textContent = missing.length ? 'The authorization request is incomplete.' : 'This browser does not support passkeys.';
    button.disabled = true;
    return;
  }
  const decode = (value) => {
    const normalized = value.replace(/-/g, '+').replace(/_/g, '/');
    const binary = atob(normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '='));
    return Uint8Array.from(binary, (character) => character.charCodeAt(0)).buffer;
  };
  const encode = (value) => {
    if (value === null) return null;
    const bytes = value instanceof ArrayBuffer ? new Uint8Array(value) : new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    let binary = '';
    bytes.forEach((byte) => { binary += String.fromCharCode(byte); });
    return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
  };
  const options = (value) => {
    const json = value.publicKey || value;
    if (typeof PublicKeyCredential.parseRequestOptionsFromJSON === 'function') return PublicKeyCredential.parseRequestOptionsFromJSON(json);
    return { ...json, challenge: decode(json.challenge), allowCredentials: (json.allowCredentials || []).map((item) => ({ ...item, id: decode(item.id) })) };
  };
  const serialize = (credential) => {
    if (typeof credential.toJSON === 'function') return credential.toJSON();
    return {
      id: credential.id, rawId: encode(credential.rawId), type: credential.type,
      authenticatorAttachment: credential.authenticatorAttachment,
      clientExtensionResults: credential.getClientExtensionResults(),
      response: {
        clientDataJSON: encode(credential.response.clientDataJSON),
        authenticatorData: encode(credential.response.authenticatorData),
        signature: encode(credential.response.signature),
        userHandle: encode(credential.response.userHandle),
      },
    };
  };
  const post = async (path, body) => {
    const response = await fetch(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body), credentials: 'omit' });
    const payload = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(payload.message || 'The authorization server rejected the request.');
    return payload;
  };
  button.addEventListener('click', async () => {
    button.disabled = true; error.textContent = ''; status.textContent = 'Waiting for your passkey…';
    try {
      const challenge = await post('/api/v1/auth/native/passkey/start', {
        email: params.get('email'), device_id: params.get('device_id'),
        code_challenge: params.get('code_challenge'), code_challenge_method: 'S256',
        state: params.get('state'), redirect_uri: 'open-cowork://auth/callback',
      });
      const credential = await navigator.credentials.get({ publicKey: options(challenge.public_key) });
      if (!credential) throw new Error('The passkey ceremony was canceled.');
      status.textContent = 'Returning to Open Cowork…';
      const result = await post('/api/v1/auth/native/passkey/finish', {
        challenge_id: challenge.challenge_id, authorization_id: challenge.authorization_id,
        credential: serialize(credential),
      });
      location.assign(result.redirect_url);
    } catch (cause) {
      status.textContent = ''; error.textContent = cause instanceof Error ? cause.message : String(cause); button.disabled = false;
    }
  });
})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_rp_id_to_match_the_public_origin() {
        let valid = PasskeyConfig {
            rp_id: "example.test".to_owned(),
            origin: "https://cowork.example.test".to_owned(),
        };
        assert!(build_webauthn(&valid).is_ok());
        let invalid = PasskeyConfig {
            rp_id: "other.test".to_owned(),
            origin: "https://cowork.example.test".to_owned(),
        };
        assert!(build_webauthn(&invalid).is_err());
    }

    #[test]
    fn native_state_is_strict_base64url() {
        assert!(validate_native_state(&"a".repeat(43)).is_ok());
        assert!(validate_native_state(&"a".repeat(42)).is_err());
        assert!(validate_native_state(&format!("{}=", "a".repeat(43))).is_err());
        assert!(validate_native_state(&format!("{}&", "a".repeat(43))).is_err());
    }

    #[test]
    fn native_authorization_page_keeps_request_data_out_of_html() {
        assert!(NATIVE_AUTHORIZATION_HTML.contains("/api/v1/auth/native/passkey/client.js"));
        assert!(!NATIVE_AUTHORIZATION_HTML.contains("{{"));
        assert!(NATIVE_AUTHORIZATION_JS.contains("location.hash"));
        assert!(NATIVE_AUTHORIZATION_JS.contains(NATIVE_REDIRECT_URI));
    }
}
