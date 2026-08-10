use std::{sync::Arc, time::Instant};

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use cowork_contracts::{
    PushConfiguration, PushSubscriptionRecord, PushSubscriptionRegistration,
    RegisterPushSubscriptionRequest, SCHEMA_VERSION,
};
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::sync::Mutex;
use uuid::Uuid;
use web_push_native::{
    jwt_simple::algorithms::ES256KeyPair, p256::PublicKey, Auth, WebPushBuilder,
};

use crate::{
    auth::Principal,
    config::{FcmConfig, PushConfig},
    error::ApiError,
    storage::SealedValue,
    AppState,
};

const GOOGLE_MESSAGING_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const MAX_DELIVERY_ATTEMPTS: i32 = 8;

#[derive(Clone)]
pub struct PushService {
    http: reqwest::Client,
    fcm: Option<FcmConfig>,
    web_push: Option<WebPushSender>,
    fcm_access_token: Arc<Mutex<Option<CachedAccessToken>>>,
}

#[derive(Clone)]
struct WebPushSender {
    key_pair: Arc<ES256KeyPair>,
    public_key: String,
    subject: String,
}

struct CachedAccessToken {
    value: String,
    valid_until: Instant,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
enum StoredSubscription {
    Fcm {
        token: String,
    },
    WebPush {
        endpoint: String,
        p256dh: String,
        auth: String,
    },
}

#[derive(Debug, Serialize)]
struct PushEnvelope<'a> {
    schema_version: u16,
    kind: &'static str,
    run_id: Uuid,
    event_kind: &'a str,
    sequence: i64,
    title: &'static str,
    body: &'static str,
}

#[derive(Debug)]
struct Delivery {
    id: Uuid,
    run_id: Uuid,
    user_id: Uuid,
    event_sequence: i64,
    event_kind: String,
    attempts: i32,
}

#[derive(Debug)]
struct Subscription {
    id: Uuid,
    user_id: Uuid,
    sealed: SealedValue,
}

#[derive(Debug)]
struct SendFailure {
    message: String,
    permanent: bool,
}

#[derive(Serialize)]
struct GoogleJwtClaims<'a> {
    iss: &'a str,
    scope: &'static str,
    aud: &'a str,
    iat: i64,
    exp: i64,
}

#[derive(Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    expires_in: u64,
}

fn sign_google_assertion(config: &FcmConfig, issued_at: i64) -> Result<String, SendFailure> {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = serde_json::to_vec(&GoogleJwtClaims {
        iss: &config.client_email,
        scope: GOOGLE_MESSAGING_SCOPE,
        aud: &config.token_uri,
        iat: issued_at,
        exp: issued_at + 3_600,
    })
    .map_err(|error| SendFailure {
        message: format!("FCM JWT claims could not be serialized: {error}"),
        permanent: false,
    })?;
    let payload = URL_SAFE_NO_PAD.encode(claims);
    let signing_input = format!("{header}.{payload}");
    let key =
        PKey::private_key_from_pem(config.private_key.as_bytes()).map_err(|error| SendFailure {
            message: format!("invalid FCM service-account key: {error}"),
            permanent: false,
        })?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key).map_err(|error| SendFailure {
        message: format!("FCM JWT signer initialization failed: {error}"),
        permanent: false,
    })?;
    signer
        .update(signing_input.as_bytes())
        .map_err(|error| SendFailure {
            message: format!("FCM JWT signing failed: {error}"),
            permanent: false,
        })?;
    let signature = signer.sign_to_vec().map_err(|error| SendFailure {
        message: format!("FCM JWT signing failed: {error}"),
        permanent: false,
    })?;
    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

impl PushService {
    pub fn from_config(config: &PushConfig) -> anyhow::Result<Option<Self>> {
        if config.fcm.is_none() && config.web_push.is_none() {
            return Ok(None);
        }
        let web_push = config
            .web_push
            .as_ref()
            .map(|config| {
                let bytes = URL_SAFE_NO_PAD
                    .decode(&config.vapid_private_key)
                    .map_err(|_| anyhow::anyhow!("invalid base64url VAPID private key"))?;
                if bytes.len() != 32 {
                    anyhow::bail!("VAPID private key must decode to 32 bytes");
                }
                let key_pair = ES256KeyPair::from_bytes(&bytes)
                    .map_err(|error| anyhow::anyhow!("invalid VAPID key: {error}"))?;
                let secret_key = web_push_native::p256::SecretKey::from_slice(&bytes)
                    .map_err(|error| anyhow::anyhow!("invalid VAPID scalar: {error}"))?;
                let public_key = URL_SAFE_NO_PAD.encode(secret_key.public_key().to_sec1_bytes());
                Ok(WebPushSender {
                    key_pair: Arc::new(key_pair),
                    public_key,
                    subject: config.subject.clone(),
                })
            })
            .transpose()?;
        Ok(Some(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()?,
            fcm: config.fcm.clone(),
            web_push,
            fcm_access_token: Arc::new(Mutex::new(None)),
        }))
    }

    fn configuration(&self) -> PushConfiguration {
        PushConfiguration {
            schema_version: SCHEMA_VERSION,
            fcm_enabled: self.fcm.is_some(),
            web_push_public_key: self
                .web_push
                .as_ref()
                .map(|sender| sender.public_key.clone()),
        }
    }

    async fn send(
        &self,
        subscription: StoredSubscription,
        envelope: &PushEnvelope<'_>,
    ) -> Result<(), SendFailure> {
        match subscription {
            StoredSubscription::Fcm { token } => self.send_fcm(&token, envelope).await,
            StoredSubscription::WebPush {
                endpoint,
                p256dh,
                auth,
            } => {
                self.send_web_push(&endpoint, &p256dh, &auth, envelope)
                    .await
            }
        }
    }

    async fn send_fcm(
        &self,
        registration_token: &str,
        envelope: &PushEnvelope<'_>,
    ) -> Result<(), SendFailure> {
        let config = self.fcm.as_ref().ok_or_else(|| SendFailure {
            message: "FCM is not configured".to_owned(),
            permanent: false,
        })?;
        let access_token = self.fcm_access_token(config).await?;
        let url = format!(
            "https://fcm.googleapis.com/v1/projects/{}/messages:send",
            config.project_id
        );
        let response = self
            .http
            .post(url)
            .bearer_auth(access_token)
            .json(&serde_json::json!({
                "message": {
                    "token": registration_token,
                    "data": {
                        "schema_version": envelope.schema_version.to_string(),
                        "kind": envelope.kind,
                        "run_id": envelope.run_id.to_string(),
                        "event_kind": envelope.event_kind,
                        "sequence": envelope.sequence.to_string()
                    },
                    "notification": {
                        "title": envelope.title,
                        "body": envelope.body
                    },
                    "android": { "priority": "normal", "ttl": "86400s" }
                }
            }))
            .send()
            .await
            .map_err(|error| SendFailure {
                message: format!("FCM request failed: {error}"),
                permanent: false,
            })?;
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(SendFailure {
            permanent: matches!(status.as_u16(), 400 | 404)
                && (body.contains("UNREGISTERED") || body.contains("INVALID_ARGUMENT")),
            message: format!("FCM returned {status}: {}", truncate_error(&body)),
        })
    }

    async fn fcm_access_token(&self, config: &FcmConfig) -> Result<String, SendFailure> {
        let mut cached = self.fcm_access_token.lock().await;
        if let Some(token) = cached.as_ref() {
            if token.valid_until > Instant::now() + std::time::Duration::from_secs(60) {
                return Ok(token.value.clone());
            }
        }
        let now = Utc::now().timestamp();
        let assertion = sign_google_assertion(config, now)?;
        let response = self
            .http
            .post(&config.token_uri)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
            ])
            .send()
            .await
            .map_err(|error| SendFailure {
                message: format!("FCM OAuth request failed: {error}"),
                permanent: false,
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(SendFailure {
                message: format!("FCM OAuth returned {status}: {}", truncate_error(&body)),
                permanent: false,
            });
        }
        let token: GoogleTokenResponse = response.json().await.map_err(|error| SendFailure {
            message: format!("FCM OAuth response was invalid: {error}"),
            permanent: false,
        })?;
        let valid_for = token.expires_in.saturating_sub(30).max(30);
        *cached = Some(CachedAccessToken {
            value: token.access_token.clone(),
            valid_until: Instant::now() + std::time::Duration::from_secs(valid_for),
        });
        Ok(token.access_token)
    }

    async fn send_web_push(
        &self,
        endpoint: &str,
        p256dh: &str,
        auth: &str,
        envelope: &PushEnvelope<'_>,
    ) -> Result<(), SendFailure> {
        let sender = self.web_push.as_ref().ok_or_else(|| SendFailure {
            message: "WebPush is not configured".to_owned(),
            permanent: false,
        })?;
        let endpoint = endpoint.parse().map_err(|_| SendFailure {
            message: "stored WebPush endpoint is invalid".to_owned(),
            permanent: true,
        })?;
        let public_key =
            PublicKey::from_sec1_bytes(&URL_SAFE_NO_PAD.decode(p256dh).map_err(|_| {
                SendFailure {
                    message: "stored WebPush p256dh key is invalid".to_owned(),
                    permanent: true,
                }
            })?)
            .map_err(|_| SendFailure {
                message: "stored WebPush p256dh key is invalid".to_owned(),
                permanent: true,
            })?;
        let auth = URL_SAFE_NO_PAD.decode(auth).map_err(|_| SendFailure {
            message: "stored WebPush auth secret is invalid".to_owned(),
            permanent: true,
        })?;
        let request = WebPushBuilder::new(endpoint, public_key, Auth::clone_from_slice(&auth))
            .with_vapid(&sender.key_pair, &sender.subject)
            .build(serde_json::to_vec(envelope).map_err(|error| SendFailure {
                message: format!("push payload serialization failed: {error}"),
                permanent: false,
            })?)
            .map_err(|error| SendFailure {
                message: format!("WebPush encryption failed: {error}"),
                permanent: true,
            })?;
        let (parts, body) = request.into_parts();
        let mut outgoing = self.http.request(parts.method, parts.uri.to_string());
        for (name, value) in &parts.headers {
            outgoing = outgoing.header(name, value);
        }
        let response = outgoing
            .body(body)
            .send()
            .await
            .map_err(|error| SendFailure {
                message: format!("WebPush request failed: {error}"),
                permanent: false,
            })?;
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        Err(SendFailure {
            message: format!("WebPush returned {status}"),
            permanent: matches!(status.as_u16(), 404 | 410),
        })
    }
}

pub async fn configuration(State(state): State<AppState>) -> Json<PushConfiguration> {
    Json(
        state
            .push
            .as_ref()
            .map(|service| service.configuration())
            .unwrap_or(PushConfiguration {
                schema_version: SCHEMA_VERSION,
                fcm_enabled: false,
                web_push_public_key: None,
            }),
    )
}

pub async fn register(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<RegisterPushSubscriptionRequest>,
) -> Result<(StatusCode, Json<PushSubscriptionRecord>), ApiError> {
    let service = state
        .push
        .as_ref()
        .ok_or_else(|| ApiError::Conflict("push delivery is not configured".to_owned()))?;
    let (provider, endpoint_identity, stored) = match request.subscription {
        PushSubscriptionRegistration::Fcm { token } => {
            if service.fcm.is_none() {
                return Err(ApiError::Conflict("FCM is not configured".to_owned()));
            }
            validate_fcm_token(&token)?;
            ("fcm", token.clone(), StoredSubscription::Fcm { token })
        }
        PushSubscriptionRegistration::WebPush {
            endpoint,
            p256dh,
            auth,
        } => {
            if service.web_push.is_none() {
                return Err(ApiError::Conflict("WebPush is not configured".to_owned()));
            }
            validate_web_subscription(&endpoint, &p256dh, &auth)?;
            (
                "web_push",
                endpoint.clone(),
                StoredSubscription::WebPush {
                    endpoint,
                    p256dh,
                    auth,
                },
            )
        }
    };
    let store = state.object_store.as_ref().ok_or_else(|| {
        ApiError::Conflict("encrypted object storage is required for push subscriptions".to_owned())
    })?;
    let sealed = store.seal_for_user(
        principal.user_id,
        &serde_json::to_vec(&stored).map_err(ApiError::from)?,
    )?;
    let endpoint_hash = Sha256::digest(format!("{provider}\0{endpoint_identity}").as_bytes());
    let id = Uuid::new_v4();
    let row = sqlx::query(
        r#"
        INSERT INTO push_subscriptions (
            id, user_id, device_id, provider, endpoint_hash, ciphertext,
            encrypted_data_key, nonce, wrap_nonce
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (user_id, provider, endpoint_hash) DO UPDATE SET
            device_id = EXCLUDED.device_id,
            ciphertext = EXCLUDED.ciphertext,
            encrypted_data_key = EXCLUDED.encrypted_data_key,
            nonce = EXCLUDED.nonce,
            wrap_nonce = EXCLUDED.wrap_nonce,
            failures = 0,
            last_error = NULL,
            revoked_at = NULL,
            updated_at = now()
        RETURNING id, device_id, provider, created_at, last_success_at
        "#,
    )
    .bind(id)
    .bind(principal.user_id)
    .bind(request.device_id)
    .bind(provider)
    .bind(endpoint_hash.as_slice())
    .bind(sealed.ciphertext)
    .bind(sealed.encrypted_data_key)
    .bind(sealed.nonce.as_slice())
    .bind(sealed.wrap_nonce.as_slice())
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(row_to_record(&row)?)))
}

pub async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<PushSubscriptionRecord>>, ApiError> {
    let rows = sqlx::query(
        "SELECT id, device_id, provider, created_at, last_success_at FROM push_subscriptions WHERE user_id = $1 AND revoked_at IS NULL ORDER BY created_at",
    )
    .bind(principal.user_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.iter()
            .map(row_to_record)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

pub async fn remove(
    State(state): State<AppState>,
    Path(subscription_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, ApiError> {
    let result = sqlx::query(
        "UPDATE push_subscriptions SET revoked_at = now(), updated_at = now() WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(subscription_id)
    .bind(principal.user_id)
    .execute(&state.pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!(
            "push subscription {subscription_id} was not found"
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn run_dispatcher(state: AppState) {
    loop {
        if let Err(error) = dispatch_batch(&state).await {
            tracing::warn!(?error, "push dispatch batch failed");
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

async fn dispatch_batch(state: &AppState) -> Result<(), ApiError> {
    let Some(service) = state.push.as_ref() else {
        return Ok(());
    };
    for _ in 0..25 {
        let Some(delivery) = claim_delivery(state).await? else {
            break;
        };
        let rows = sqlx::query(
            "SELECT id, user_id, ciphertext, encrypted_data_key, nonce, wrap_nonce FROM push_subscriptions WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(delivery.user_id)
        .fetch_all(&state.pool)
        .await?;
        let subscriptions = rows
            .iter()
            .map(row_to_subscription)
            .collect::<Result<Vec<_>, _>>()?;
        if subscriptions.is_empty() {
            mark_delivery_success(&state.pool, delivery.id).await?;
            continue;
        }
        let envelope = privacy_safe_envelope(&delivery);
        let mut successes = 0_u32;
        let mut errors = Vec::new();
        for subscription in subscriptions {
            let result = decrypt_subscription(state, &subscription)
                .and_then(|value| serde_json::from_slice(&value).map_err(ApiError::from));
            let result = match result {
                Ok(subscription_data) => service.send(subscription_data, &envelope).await,
                Err(error) => Err(SendFailure {
                    message: error.to_string(),
                    permanent: true,
                }),
            };
            match result {
                Ok(()) => {
                    successes += 1;
                    sqlx::query("UPDATE push_subscriptions SET failures = 0, last_error = NULL, last_success_at = now(), updated_at = now() WHERE id = $1")
                        .bind(subscription.id)
                        .execute(&state.pool)
                        .await?;
                }
                Err(error) => {
                    errors.push(error.message.clone());
                    sqlx::query("UPDATE push_subscriptions SET failures = failures + 1, last_error = $2, updated_at = now(), revoked_at = CASE WHEN $3 THEN now() ELSE revoked_at END WHERE id = $1")
                        .bind(subscription.id)
                        .bind(truncate_error(&error.message))
                        .bind(error.permanent)
                        .execute(&state.pool)
                        .await?;
                }
            }
        }
        if successes > 0 {
            mark_delivery_success(&state.pool, delivery.id).await?;
        } else {
            mark_delivery_failure(&state.pool, &delivery, &errors.join("; ")).await?;
        }
    }
    Ok(())
}

async fn claim_delivery(state: &AppState) -> Result<Option<Delivery>, ApiError> {
    let row = sqlx::query(
        r#"
        WITH candidate AS (
            SELECT id FROM push_deliveries
            WHERE (state = 'queued' AND next_attempt_at <= now())
               OR (state = 'processing' AND updated_at < now() - interval '5 minutes')
            ORDER BY created_at
            LIMIT 1 FOR UPDATE SKIP LOCKED
        )
        UPDATE push_deliveries delivery
        SET state = 'processing', updated_at = now()
        FROM candidate WHERE delivery.id = candidate.id
        RETURNING delivery.id, delivery.run_id, delivery.user_id,
                  delivery.event_sequence, delivery.event_kind, delivery.attempts
        "#,
    )
    .fetch_optional(&state.pool)
    .await?;
    row.map(|row| {
        Ok(Delivery {
            id: row.try_get("id")?,
            run_id: row.try_get("run_id")?,
            user_id: row.try_get("user_id")?,
            event_sequence: row.try_get("event_sequence")?,
            event_kind: row.try_get("event_kind")?,
            attempts: row.try_get("attempts")?,
        })
    })
    .transpose()
}

fn privacy_safe_envelope(delivery: &Delivery) -> PushEnvelope<'_> {
    PushEnvelope {
        schema_version: SCHEMA_VERSION,
        kind: "run_event",
        run_id: delivery.run_id,
        event_kind: &delivery.event_kind,
        sequence: delivery.event_sequence,
        title: "Open Cowork",
        body: "A run needs your attention.",
    }
}

async fn mark_delivery_success(pool: &sqlx::PgPool, id: Uuid) -> Result<(), ApiError> {
    sqlx::query("UPDATE push_deliveries SET state = 'delivered', delivered_at = now(), updated_at = now(), last_error = NULL WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn mark_delivery_failure(
    pool: &sqlx::PgPool,
    delivery: &Delivery,
    error: &str,
) -> Result<(), ApiError> {
    let attempts = delivery.attempts + 1;
    let failed = attempts >= MAX_DELIVERY_ATTEMPTS;
    let delay_seconds = (5_i64 * 2_i64.pow(attempts.clamp(0, 10) as u32)).min(3_600);
    sqlx::query(
        "UPDATE push_deliveries SET state = $2, attempts = $3, next_attempt_at = now() + make_interval(secs => $4), last_error = $5, updated_at = now() WHERE id = $1",
    )
    .bind(delivery.id)
    .bind(if failed { "failed" } else { "queued" })
    .bind(attempts)
    .bind(delay_seconds as f64)
    .bind(truncate_error(error))
    .execute(pool)
    .await?;
    Ok(())
}

fn decrypt_subscription(state: &AppState, row: &Subscription) -> Result<Vec<u8>, ApiError> {
    let store = state.object_store.as_ref().ok_or_else(|| {
        ApiError::Conflict("encrypted object storage is required for push subscriptions".to_owned())
    })?;
    store.open_for_user(row.user_id, &row.sealed)
}

fn row_to_subscription(row: &sqlx::postgres::PgRow) -> Result<Subscription, ApiError> {
    Ok(Subscription {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        sealed: SealedValue {
            ciphertext: row.try_get("ciphertext")?,
            encrypted_data_key: row.try_get("encrypted_data_key")?,
            nonce: fixed_nonce(row.try_get("nonce")?)?,
            wrap_nonce: fixed_nonce(row.try_get("wrap_nonce")?)?,
        },
    })
}

fn fixed_nonce(value: Vec<u8>) -> Result<[u8; 12], ApiError> {
    value
        .try_into()
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("stored push nonce is invalid")))
}

fn row_to_record(row: &sqlx::postgres::PgRow) -> Result<PushSubscriptionRecord, ApiError> {
    Ok(PushSubscriptionRecord {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        device_id: row.try_get("device_id")?,
        provider: row.try_get("provider")?,
        created_at: row.try_get("created_at")?,
        last_success_at: row.try_get("last_success_at")?,
    })
}

fn validate_fcm_token(token: &str) -> Result<(), ApiError> {
    if !(20..=4_096).contains(&token.len()) || token.chars().any(char::is_control) {
        return Err(ApiError::Unprocessable(
            "FCM registration token is invalid".to_owned(),
        ));
    }
    Ok(())
}

fn validate_web_subscription(endpoint: &str, p256dh: &str, auth: &str) -> Result<(), ApiError> {
    let url = reqwest::Url::parse(endpoint)
        .map_err(|_| ApiError::Unprocessable("WebPush endpoint is invalid".to_owned()))?;
    if url.scheme() != "https" || endpoint.len() > 4_096 {
        return Err(ApiError::Unprocessable(
            "WebPush endpoint must use HTTPS".to_owned(),
        ));
    }
    let public_key = URL_SAFE_NO_PAD
        .decode(p256dh)
        .map_err(|_| ApiError::Unprocessable("WebPush p256dh key is invalid".to_owned()))?;
    PublicKey::from_sec1_bytes(&public_key)
        .map_err(|_| ApiError::Unprocessable("WebPush p256dh key is invalid".to_owned()))?;
    let auth = URL_SAFE_NO_PAD
        .decode(auth)
        .map_err(|_| ApiError::Unprocessable("WebPush auth secret is invalid".to_owned()))?;
    if auth.len() != 16 {
        return Err(ApiError::Unprocessable(
            "WebPush auth secret must contain 16 bytes".to_owned(),
        ));
    }
    Ok(())
}

fn truncate_error(value: &str) -> String {
    value.chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl::{rsa::Rsa, sign::Verifier};

    #[test]
    fn push_payload_has_no_prompt_or_file_content() {
        let delivery = Delivery {
            id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            event_sequence: 7,
            event_kind: "input_requested".to_owned(),
            attempts: 0,
        };
        let json = serde_json::to_value(privacy_safe_envelope(&delivery)).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 7);
        assert_eq!(json["event_kind"], "input_requested");
        let encoded = json.to_string();
        assert!(!encoded.contains("prompt"));
        assert!(!encoded.contains("file"));
        assert!(!encoded.contains("payload"));
    }

    #[test]
    fn validates_subscription_boundaries() {
        assert!(validate_fcm_token(&"x".repeat(100)).is_ok());
        assert!(validate_fcm_token("short").is_err());
        assert!(validate_web_subscription("http://push.invalid", "x", "x").is_err());
    }

    #[test]
    fn fcm_assertion_is_an_rs256_jwt_with_a_valid_openssl_signature() {
        let rsa = Rsa::generate(2_048).unwrap();
        let private_key = String::from_utf8(rsa.private_key_to_pem().unwrap()).unwrap();
        let public_key =
            PKey::from_rsa(Rsa::public_key_from_pem(&rsa.public_key_to_pem().unwrap()).unwrap())
                .unwrap();
        let config = FcmConfig {
            project_id: "test-project".to_owned(),
            client_email: "push@opencowork.invalid".to_owned(),
            private_key,
            token_uri: "https://oauth2.googleapis.com/token".to_owned(),
        };
        let assertion = sign_google_assertion(&config, 1_700_000_000).unwrap();
        let segments = assertion.split('.').collect::<Vec<_>>();
        assert_eq!(segments.len(), 3);
        let header: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(segments[0]).unwrap()).unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(segments[1]).unwrap()).unwrap();
        assert_eq!(header, serde_json::json!({"alg":"RS256","typ":"JWT"}));
        assert_eq!(claims["iss"], config.client_email);
        assert_eq!(claims["iat"], 1_700_000_000_i64);
        assert_eq!(claims["exp"], 1_700_003_600_i64);
        let mut verifier = Verifier::new(MessageDigest::sha256(), &public_key).unwrap();
        verifier
            .update(format!("{}.{}", segments[0], segments[1]).as_bytes())
            .unwrap();
        assert!(verifier
            .verify(&URL_SAFE_NO_PAD.decode(segments[2]).unwrap())
            .unwrap());
    }
}
