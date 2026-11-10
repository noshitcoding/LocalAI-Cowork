//! Original provider flows. Only this SDK consumer can turn a signed subject into a session.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_contracts::{
    AndroidOidcBegin, AndroidOidcComplete, AndroidOidcConfig, AndroidOidcFlow, AndroidOidcLink,
    AndroidOidcOrigin, AndroidOidcProfile, AndroidOidcProfileWrite, AndroidOidcRecover,
};
use cowork_secrets::{KeyId, KeyPurpose, KeyStore};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, HttpRequest, HttpResponse, IssuerUrl,
    Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata, CoreTokenResponse},
};
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::{Zeroize, Zeroizing};
#[path = "android_oidc_cipher.rs"]
mod cipher;
#[path = "android_oidc_compat.rs"]
mod compat;
#[path = "android_oidc_origins.rs"]
mod origins;
#[path = "android_oidc_profiles.rs"]
mod profiles;
#[path = "android_oidc_sso.rs"]
mod sso;
use super::android_oidc_verifier::{VerificationContext, verify};
pub(super) fn digest(bytes: &[u8]) -> String {
    hash(bytes)
}
fn random() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn resume(secret: &str) -> Result<()> {
    if secret.len() != 64 || !secret.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(IdentityError::Unauthenticated);
    }
    Ok(())
}
fn mac(keys: &dyn KeyStore, key: KeyId, value: &impl Serialize) -> Result<String> {
    let key = keys
        .key_for_purpose(&key, KeyPurpose::RequestFingerprint)
        .map_err(|_| IdentityError::NativeAuditKeyUnavailable)?;
    let material = keys
        .load(&key)
        .map_err(|_| IdentityError::NativeAuditKeyUnavailable)?;
    let mut context = ring::hmac::Context::with_key(&ring::hmac::Key::new(
        ring::hmac::HMAC_SHA256,
        material.as_ref(),
    ));
    context.update(b"oidc-original-operation/v1\0");
    context.update(encoded(value)?.as_bytes());
    Ok(context
        .sign()
        .as_ref()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect())
}
fn url(value: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).map_err(|_| IdentityError::Validation)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || value.len() > 2048
    {
        return Err(IdentityError::Validation);
    }
    Ok(url)
}
fn config(value: &AndroidOidcConfig) -> Result<()> {
    url(&value.issuer)?;
    if value.client_id.is_empty()
        || value.client_id.len() > 512
        || value.client_id.chars().any(char::is_control)
        || value.redirect_uris.is_empty()
        || value.redirect_uris.len() > 16
        || value.endpoint_origins.len() > 16
        || value.groups_claim.is_empty()
        || value.groups_claim.len() > 256
        || value.groups_claim.chars().any(char::is_control)
    {
        return Err(IdentityError::Validation);
    }
    for origin in &value.endpoint_origins {
        if url(origin)?.origin().ascii_serialization() != *origin {
            return Err(IdentityError::Validation);
        }
    }
    for value in &value.redirect_uris {
        let parsed = reqwest::Url::parse(value).map_err(|_| IdentityError::Validation)?;
        if value.len() > 2048
            || parsed.fragment().is_some()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || !matches!(parsed.scheme(), "https" | "localai-cowork" | "cowork")
                && !(parsed.scheme() == "http"
                    && matches!(parsed.host_str(), Some("127.0.0.1" | "[::1]")))
        {
            return Err(IdentityError::Validation);
        }
    }
    Ok(())
}
fn endpoint_allowed(config: &AndroidOidcConfig, target: &str) -> Result<bool> {
    let original = url(&config.issuer)?;
    let target = url(target)?;
    Ok(target.origin() == original.origin()
        || config
            .endpoint_origins
            .iter()
            .any(|value| *value == target.origin().ascii_serialization()))
}
async fn http(config: AndroidOidcConfig, request: HttpRequest) -> Result<HttpResponse> {
    let target = url(&request.uri().to_string())?;
    if !endpoint_allowed(&config, target.as_str())? {
        return Err(IdentityError::Forbidden);
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .map_err(|_| IdentityError::Unauthenticated)?;
    let mut response = client
        .request(request.method().clone(), target)
        .headers(request.headers().clone())
        .body(request.body().clone())
        .send()
        .await
        .map_err(|_| IdentityError::Unauthenticated)?;
    let status = response.status();
    let headers = response.headers().clone();
    if headers
        .iter()
        .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
        .sum::<usize>()
        > 8192
    {
        return Err(IdentityError::QuotaExceeded);
    }
    let mut body = Vec::new();
    while let Some(bytes) = response
        .chunk()
        .await
        .map_err(|_| IdentityError::Unauthenticated)?
    {
        if body
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > 65536)
        {
            return Err(IdentityError::QuotaExceeded);
        }
        body.extend_from_slice(&bytes)
    }
    let mut result = axum::http::Response::builder()
        .status(status)
        .body(body)
        .map_err(|_| IdentityError::Corrupt)?;
    *result.headers_mut() = headers;
    Ok(result)
}
async fn discover(profile: &AndroidOidcProfile) -> Result<CoreProviderMetadata> {
    let issuer = profile.config.issuer.clone();
    let client = |request| http(profile.config.clone(), request);
    let metadata = CoreProviderMetadata::discover_async(
        IssuerUrl::new(issuer.clone()).map_err(|_| IdentityError::Validation)?,
        &client,
    )
    .await
    .map_err(|_| IdentityError::Unauthenticated)?;
    if metadata.issuer().as_str() != issuer
        || !endpoint_allowed(&profile.config, metadata.authorization_endpoint().as_str())?
        || metadata
            .token_endpoint()
            .is_none_or(|v| !endpoint_allowed(&profile.config, v.as_str()).unwrap_or(false))
    {
        return Err(IdentityError::Forbidden);
    }
    Ok(metadata)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlowPayload {
    metadata: CoreProviderMetadata,
    nonce: String,
    pkce_verifier: String,
    redirect_uri: String,
    authorization_url: String,
    begin: AndroidOidcBegin,
}
#[derive(Clone)]
struct CapturingHttp {
    store: IdentityStore,
    profile: AndroidOidcProfile,
    flow_id: Uuid,
    nonce: String,
    keys: std::sync::Arc<dyn KeyStore>,
    key: KeyId,
}
impl<'c> openidconnect::AsyncHttpClient<'c> for CapturingHttp {
    type Error = IdentityError;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<HttpResponse>> + Send + 'static>>;
    fn call(&'c self, request: HttpRequest) -> Self::Future {
        let owned = self.clone();
        Box::pin(async move {
            let response = http(owned.profile.config.clone(), request).await?;
            let original_http = OriginalHttp {
                status: response.status().as_u16(),
                headers: response
                    .headers()
                    .iter()
                    .map(|(name, value)| (name.as_str().into(), STANDARD.encode(value.as_bytes())))
                    .collect(),
                body_base64: STANDARD.encode(response.body()),
            };
            let original_bytes = Zeroizing::new(
                serde_json::to_vec(&original_http).map_err(|_| IdentityError::Corrupt)?,
            );
            let mut tx = owned.store.begin().await?;
            let actual=sqlx::query("SELECT * FROM android_oidc_exchanges WHERE flow_id=$1 AND dispatch_nonce=$2 AND response_id IS NULL AND state IN('dispatched','retired_unknown')").bind(owned.flow_id.to_string()).bind(&owned.nonce).fetch_optional(&mut *tx).await?.ok_or(IdentityError::Conflict)?;
            let at = owned.store.sample(&mut tx).await?;
            let id = cipher::put(
                &mut tx,
                owned.keys.as_ref(),
                owned.key,
                &owned.profile,
                Uuid::new_v4(),
                "oauth_response",
                &original_bytes,
                at,
            )
            .await?;
            if sqlx::query("UPDATE android_oidc_exchanges SET state=CASE WHEN state='retired_unknown' THEN 'received_retired' ELSE 'received' END,response_id=$1,response_sha256=$2,response_status=$3,received_at=$4 WHERE flow_id=$5 AND dispatch_nonce=$6 AND response_id IS NULL AND request_sha256=$7").bind(id.to_string()).bind(digest(&original_bytes)).bind(i64::from(response.status().as_u16())).bind(at).bind(owned.flow_id.to_string()).bind(&owned.nonce).bind(actual.try_get::<String,_>("request_sha256")?).execute(&mut *tx).await?.rows_affected()!=1{return Err(IdentityError::Conflict)}
            sqlx::query(
                "UPDATE android_oidc_flows SET phase='received' WHERE id=$1 AND phase='dispatched'",
            )
            .bind(owned.flow_id.to_string())
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok(response)
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginalHttp {
    status: u16,
    headers: Vec<(String, String)>,
    body_base64: String,
}
impl Drop for OriginalHttp {
    fn drop(&mut self) {
        self.body_base64.zeroize();
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptBytes {
    access_token: String,
    user_id: Uuid,
    session_id: Uuid,
    expires_at: i64,
    flow_id: Uuid,
    claims_sha256: String,
    linked: bool,
}
impl Drop for ReceiptBytes {
    fn drop(&mut self) {
        self.access_token.zeroize();
    }
}
/// Not deserializable or constructible from HTTP. Its bearer was issued in the
/// same verified flow transaction as the persisted original session origin.
#[derive(Serialize)]
pub struct OidcSessionReceipt {
    inner: ReceiptBytes,
}
impl OidcSessionReceipt {
    pub fn access_token(&self) -> &str {
        &self.inner.access_token
    }
    pub fn identity_id(&self) -> Uuid {
        self.inner.user_id
    }
    pub fn session_id(&self) -> Uuid {
        self.inner.session_id
    }
    pub fn expires_at(&self) -> i64 {
        self.inner.expires_at
    }
    pub fn flow_id(&self) -> Uuid {
        self.inner.flow_id
    }
}
async fn flow(tx: &mut Transaction<'_, Any>, id: Uuid, secret: &str) -> Result<sqlx::any::AnyRow> {
    resume(secret)?;
    let row = sqlx::query("SELECT * FROM android_oidc_flows WHERE id=$1 AND resume_sha256=$2")
        .bind(id.to_string())
        .bind(secret_hash("oidc-resume", secret))
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(IdentityError::Unauthenticated)?;
    Ok(row)
}
impl IdentityStore {
    pub async fn android_oidc_retire(
        &self,
        q: &cowork_contracts::AndroidOidcRetire,
        keys: std::sync::Arc<dyn KeyStore>,
        key: KeyId,
    ) -> Result<Value> {
        if q.request_id.is_nil() {
            return Err(IdentityError::Validation);
        }
        let fingerprint = mac(keys.as_ref(), key, q)?;
        let mut tx = self.begin().await?;
        let f = flow(&mut tx, q.flow_id, &q.resume_secret).await?;
        let p = profiles::read(&mut tx, identifier(f.try_get("profile_id")?)?).await?;
        if let Some(old) = sqlx::query("SELECT * FROM android_oidc_flow_closures WHERE flow_id=$1")
            .bind(q.flow_id.to_string())
            .fetch_optional(&mut *tx)
            .await?
        {
            if old.try_get::<String, _>("request_mac")? != fingerprint {
                return Err(IdentityError::Conflict);
            }
            let out: Value = serde_json::from_slice(
                &cipher::read(
                    &mut tx,
                    keys.as_ref(),
                    identifier(old.try_get("receipt_id")?)?,
                    p.id,
                    "oauth_response",
                )
                .await?,
            )
            .map_err(|_| IdentityError::Corrupt)?;
            tx.commit().await?;
            return Ok(out);
        }
        let phase: String = f.try_get("phase")?;
        if phase == "committed" || phase == "retired" {
            return Err(IdentityError::Conflict);
        }
        let exchange = sqlx::query(
            "SELECT dispatched_at,response_id FROM android_oidc_exchanges WHERE flow_id=$1",
        )
        .bind(q.flow_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let now = self.sample(&mut tx).await?;
        let unknown = exchange.as_ref().is_some_and(|r| {
            r.try_get::<Option<String>, _>("response_id")
                .ok()
                .flatten()
                .is_none()
        });
        if unknown
            && exchange.as_ref().is_some_and(|r| {
                r.try_get::<i64, _>("dispatched_at")
                    .ok()
                    .is_none_or(|at| now < at.saturating_add(30000))
            })
        {
            return Err(IdentityError::Conflict);
        }
        let out = json!({"flow_id":q.flow_id,"request_id":q.request_id,"original_phase":phase,"retired":true,"external_effect_disposition":if unknown{"unknown_not_replayed"}else{"original_unsent_or_response_retained"},"external_tokens_revoked":false,"replacement_session_issued":false,"closed_at":now});
        let record = cipher::put(
            &mut tx,
            keys.as_ref(),
            key,
            &p,
            Uuid::new_v4(),
            "oauth_response",
            &serde_json::to_vec(&out).map_err(|_| IdentityError::Corrupt)?,
            now,
        )
        .await?;
        let current = self.sample(&mut tx).await?;
        if unknown
            && exchange.as_ref().is_some_and(|r| {
                r.try_get::<i64, _>("dispatched_at")
                    .ok()
                    .is_none_or(|at| current < at.saturating_add(30000))
            })
        {
            return Err(IdentityError::Conflict);
        }
        sqlx::query("INSERT INTO android_oidc_flow_closures(flow_id,request_id,request_mac,receipt_id,created_at) VALUES($1,$2,$3,$4,$5)").bind(q.flow_id.to_string()).bind(q.request_id.to_string()).bind(fingerprint).bind(record.to_string()).bind(current).execute(&mut *tx).await?;
        if sqlx::query("UPDATE android_oidc_flows SET phase='retired' WHERE id=$1 AND phase=$2")
            .bind(q.flow_id.to_string())
            .bind(phase)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            != 1
        {
            return Err(IdentityError::Conflict);
        }
        sqlx::query("UPDATE android_oidc_exchanges SET state='retired_unknown' WHERE flow_id=$1 AND state='dispatched' AND response_id IS NULL").bind(q.flow_id.to_string()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(out)
    }
    async fn current_oidc_flow(
        &self,
        tx: &mut Transaction<'_, Any>,
        row: &sqlx::any::AnyRow,
        expired_ok: bool,
    ) -> Result<AndroidOidcProfile> {
        let profile = profiles::read(tx, identifier(row.try_get("profile_id")?)?).await?;
        let now = self.sample(tx).await?;
        if !profile.active
            || profile.revision != row.try_get::<i64, _>("profile_revision")?
            || profile.epoch.to_string() != row.try_get::<String, _>("profile_epoch")?
            || profile.config_sha256 != row.try_get::<String, _>("config_sha256")?
            || !expired_ok && row.try_get::<i64, _>("expires_at")? <= now
        {
            return Err(IdentityError::Unauthenticated);
        }
        if profiles::org_epoch(tx, profile.organization_id).await?
            != row.try_get::<i64, _>("organization_epoch")?
        {
            return Err(IdentityError::Forbidden);
        }
        if let Some(policy) = sso::admission(tx, &profile).await? {
            if policy.revision != row.try_get::<i64, _>("sso_revision")?
                || Some(policy.epoch.to_string())
                    != row.try_get::<Option<String>, _>("sso_epoch")?
            {
                return Err(IdentityError::Forbidden);
            }
        }
        if let Some(session) = row.try_get::<Option<String>, _>("link_session_id")? {
            let actor = Principal {
                identity_id: identifier(
                    row.try_get::<Option<String>, _>("link_owner_id")?
                        .ok_or(IdentityError::Corrupt)?,
                )?,
                session_id: identifier(session)?,
                session_revision: row.try_get("link_session_revision")?,
            };
            let identity = self.actor(tx, &actor).await?;
            if identity.revision != row.try_get::<i64, _>("link_identity_revision")? {
                return Err(IdentityError::Unauthenticated);
            }
            self.require_android_factor_verified(tx, &actor).await?;
        }
        let now = self.sample(tx).await?;
        if !expired_ok && row.try_get::<i64, _>("expires_at")? <= now {
            return Err(IdentityError::Unauthenticated);
        }
        Ok(profile)
    }
    pub async fn android_oidc_begin(
        &self,
        actor: Option<&Principal>,
        q: &AndroidOidcBegin,
        keys: std::sync::Arc<dyn KeyStore>,
        key: KeyId,
    ) -> Result<AndroidOidcFlow> {
        resume(&q.resume_secret)?;
        if q.native_code_challenge.as_ref().is_some_and(|v| {
            v.len() != 43
                || !v
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        }) {
            return Err(IdentityError::Validation);
        }
        if q.request_id.is_nil()
            || !matches!(q.purpose.as_str(), "login" | "link")
            || q.purpose == "link" && actor.is_none()
            || q.device_id.is_empty()
            || q.device_id.len() > 200
            || q.device_id.chars().any(char::is_control)
        {
            return Err(IdentityError::Validation);
        }
        let fingerprint = mac(keys.as_ref(), key, q)?;
        let mut tx = self.begin().await?;
        if let Some(old) = sqlx::query("SELECT * FROM android_oidc_flows WHERE request_id=$1")
            .bind(q.request_id.to_string())
            .fetch_optional(&mut *tx)
            .await?
        {
            if old.try_get::<String, _>("request_mac")? != fingerprint {
                return Err(IdentityError::Conflict);
            }
            if old.try_get::<String, _>("phase")? == "retired" {
                return Err(IdentityError::Conflict);
            }
            if q.purpose == "link" {
                let actual = actor.ok_or(IdentityError::Unauthenticated)?;
                if old.try_get::<Option<String>, _>("link_owner_id")?
                    != Some(actual.identity_id.to_string())
                    || old.try_get::<Option<String>, _>("link_session_id")?
                        != Some(actual.session_id.to_string())
                    || old.try_get::<i64, _>("link_session_revision")? != actual.session_revision
                {
                    return Err(IdentityError::Unauthenticated);
                }
            }
            let p = self.current_oidc_flow(&mut tx, &old, false).await?;
            let bytes = cipher::read(
                &mut tx,
                keys.as_ref(),
                identifier(old.try_get("payload_id")?)?,
                p.id,
                "oauth_flow",
            )
            .await?;
            let payload: FlowPayload =
                serde_json::from_slice(&bytes).map_err(|_| IdentityError::Corrupt)?;
            if encoded(&payload.begin)? != encoded(q)? {
                return Err(IdentityError::Conflict);
            }
            self.current_oidc_flow(&mut tx, &old, false).await?;
            let value = view(&old, &payload)?;
            tx.commit().await?;
            return Ok(value);
        }
        let p = profiles::read(&mut tx, q.profile_id).await?;
        if !p.active || !p.config.redirect_uris.contains(&q.redirect_uri) {
            return Err(IdentityError::Forbidden);
        }
        let org = profiles::org_epoch(&mut tx, p.organization_id).await?;
        let sso_policy = sso::admission(&mut tx, &p).await?;
        let linked = if q.purpose == "link" {
            let a = actor.ok_or(IdentityError::Unauthenticated)?;
            if let Some(org) = p.organization_id {
                if self
                    .session_organization_in_transaction(&mut tx, a)
                    .await?
                    .is_some_and(|scope| scope != org)
                {
                    return Err(IdentityError::Forbidden);
                }
            } else {
                self.require_global_session_in_transaction(&mut tx, a)
                    .await?;
            }
            self.require_android_factor_verified(&mut tx, a).await?;
            Some((a.clone(), self.actor(&mut tx, a).await?))
        } else {
            None
        };
        tx.commit().await?;
        let metadata = discover(&p).await?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let client = CoreClient::from_provider_metadata(
            metadata.clone(),
            ClientId::new(p.config.client_id.clone()),
            None,
        )
        .set_redirect_uri(
            RedirectUrl::new(q.redirect_uri.clone()).map_err(|_| IdentityError::Validation)?,
        );
        let (authorization, state, nonce) = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("profile".into()))
            .set_pkce_challenge(challenge)
            .url();
        let payload = FlowPayload {
            metadata,
            nonce: nonce.secret().clone(),
            pkce_verifier: verifier.secret().clone(),
            redirect_uri: q.redirect_uri.clone(),
            authorization_url: authorization.to_string(),
            begin: q.clone(),
        };
        let mut tx = self.begin().await?;
        let current = profiles::read(&mut tx, p.id).await?;
        if sso::admission(&mut tx, &current)
            .await?
            .as_ref()
            .map(|s| (s.revision, s.epoch))
            != sso_policy.as_ref().map(|s| (s.revision, s.epoch))
        {
            return Err(IdentityError::Conflict);
        }
        if current.revision != p.revision
            || current.epoch != p.epoch
            || !current.active
            || profiles::org_epoch(&mut tx, p.organization_id).await? != org
        {
            return Err(IdentityError::Conflict);
        }
        if let Some((a, value)) = &linked {
            if self.actor(&mut tx, a).await?.revision != value.revision {
                return Err(IdentityError::Unauthenticated);
            }
            self.require_android_factor_verified(&mut tx, a).await?;
        }
        let at = self.sample(&mut tx).await?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM android_oidc_flows")
            .fetch_one(&mut *tx)
            .await?;
        if count >= 100000 {
            return Err(IdentityError::QuotaExceeded);
        }
        let id = Uuid::new_v4();
        let record = cipher::put(
            &mut tx,
            keys.as_ref(),
            key,
            &p,
            Uuid::new_v4(),
            "oauth_flow",
            &serde_json::to_vec(&payload).map_err(|_| IdentityError::Corrupt)?,
            at,
        )
        .await?;
        sqlx::query("INSERT INTO android_oidc_flows(id,request_id,profile_id,profile_revision,profile_epoch,config_sha256,organization_epoch,purpose,device_id,state_sha256,resume_sha256,request_mac,link_owner_id,link_session_id,link_session_revision,link_identity_revision,payload_id,created_at,expires_at,phase,sso_revision,sso_epoch) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,'prepared',$20,$21)").bind(id.to_string()).bind(q.request_id.to_string()).bind(p.id.to_string()).bind(p.revision).bind(p.epoch.to_string()).bind(&p.config_sha256).bind(org).bind(&q.purpose).bind(&q.device_id).bind(secret_hash("oidc-state",state.secret())).bind(secret_hash("oidc-resume",&q.resume_secret)).bind(fingerprint).bind(linked.as_ref().map(|(a,_)|a.identity_id.to_string())).bind(linked.as_ref().map(|(a,_)|a.session_id.to_string())).bind(linked.as_ref().map(|(a,_)|a.session_revision).unwrap_or(0)).bind(linked.as_ref().map(|(_,v)|v.revision).unwrap_or(0)).bind(record.to_string()).bind(at).bind(at+300000).bind(sso_policy.as_ref().map(|s|s.revision).unwrap_or(0)).bind(sso_policy.as_ref().map(|s|s.epoch.to_string())).execute(&mut *tx).await?;
        let row = flow(&mut tx, id, &q.resume_secret).await?;
        self.current_oidc_flow(&mut tx, &row, false).await?;
        let result = view(&row, &payload)?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn android_oidc_recover(
        &self,
        q: &AndroidOidcRecover,
        keys: std::sync::Arc<dyn KeyStore>,
    ) -> Result<Value> {
        let mut tx = self.begin().await?;
        let row = flow(&mut tx, q.flow_id, &q.resume_secret).await?;
        let p = self.current_oidc_flow(&mut tx, &row, true).await?;
        let payload: FlowPayload = serde_json::from_slice(
            &cipher::read(
                &mut tx,
                keys.as_ref(),
                identifier(row.try_get("payload_id")?)?,
                p.id,
                "oauth_flow",
            )
            .await?,
        )
        .map_err(|_| IdentityError::Corrupt)?;
        let mut result = if row.try_get::<String, _>("phase")? == "committed" {
            json!({"flow":view(&row,&payload)?,"receipt":self.original_oidc_receipt(&mut tx,&row,keys.as_ref()).await?})
        } else {
            json!({"flow":view(&row,&payload)?,"exchange_repeat_allowed":false,"original_response_received":sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM android_oidc_exchanges WHERE flow_id=$1 AND response_id IS NOT NULL").bind(q.flow_id.to_string()).fetch_one(&mut *tx).await?==1})
        };
        result["native_code_challenge"] = json!(payload.begin.native_code_challenge);
        if let Some(denial) =
            sqlx::query("SELECT receipt_id FROM android_oidc_denials WHERE flow_id=$1")
                .bind(q.flow_id.to_string())
                .fetch_optional(&mut *tx)
                .await?
        {
            let bytes = cipher::read(
                &mut tx,
                keys.as_ref(),
                identifier(denial.try_get("receipt_id")?)?,
                p.id,
                "oauth_response",
            )
            .await?;
            result["denial_receipt"] =
                serde_json::from_slice(&bytes).map_err(|_| IdentityError::Corrupt)?;
        }
        self.current_oidc_flow(&mut tx, &row, true).await?;
        tx.commit().await?;
        Ok(result)
    }
    async fn original_oidc_receipt(
        &self,
        tx: &mut Transaction<'_, Any>,
        f: &sqlx::any::AnyRow,
        keys: &dyn KeyStore,
    ) -> Result<OidcSessionReceipt> {
        let r = sqlx::query("SELECT * FROM android_oidc_receipts WHERE flow_id=$1")
            .bind(f.try_get::<String, _>("id")?)
            .fetch_one(&mut **tx)
            .await?;
        let bytes = cipher::read(
            tx,
            keys,
            identifier(r.try_get("payload_id")?)?,
            identifier(f.try_get("profile_id")?)?,
            "oauth_token",
        )
        .await?;
        if digest(&bytes) != r.try_get::<String, _>("receipt_sha256")? {
            return Err(IdentityError::Corrupt);
        }
        let inner: ReceiptBytes =
            serde_json::from_slice(&bytes).map_err(|_| IdentityError::Corrupt)?;
        if inner.session_id.to_string() != r.try_get::<String, _>("session_id")?
            || inner.user_id.to_string() != r.try_get::<String, _>("identity_id")?
            || inner.flow_id.to_string() != f.try_get::<String, _>("id")?
        {
            return Err(IdentityError::Corrupt);
        }
        let actor = self
            .authenticate_android_token_in_transaction(tx, &inner.access_token)
            .await?;
        self.validate_oidc_origin_in_transaction(tx, &actor).await?;
        Ok(OidcSessionReceipt { inner })
    }
    pub async fn android_oidc_complete(
        &self,
        q: &AndroidOidcComplete,
        keys: std::sync::Arc<dyn KeyStore>,
        key: KeyId,
    ) -> Result<OidcSessionReceipt> {
        let owner = std::sync::Arc::new(std::sync::Mutex::new(None));
        let result = self.oidc_complete_inner(q, keys, key, owner.clone()).await;
        let original_nonce = owner.lock().map_err(|_| IdentityError::Corrupt)?.clone();
        if let Some(nonce) = original_nonce {
            let mut observed = self.begin().await?;
            let at = self.sample(&mut observed).await?;
            sqlx::query("UPDATE android_oidc_exchanges SET producer_finished_at=$1,producer_outcome=CASE WHEN response_id IS NULL THEN 'local_sdk_return_without_body' ELSE 'original_http_body_durable' END WHERE flow_id=$2 AND dispatch_nonce=$3 AND producer_finished_at=0").bind(at).bind(q.flow_id.to_string()).bind(nonce).execute(&mut *observed).await?;
            observed.commit().await?;
        }
        result
    }
    async fn oidc_complete_inner(
        &self,
        q: &AndroidOidcComplete,
        keys: std::sync::Arc<dyn KeyStore>,
        key: KeyId,
        producer: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    ) -> Result<OidcSessionReceipt> {
        if q.code.is_empty() || q.code.len() > 8192 || q.state.len() > 512 {
            return Err(IdentityError::Unauthenticated);
        }
        let mut tx = self.begin().await?;
        let f = flow(&mut tx, q.flow_id, &q.resume_secret).await?;
        if f.try_get::<String, _>("state_sha256")? != secret_hash("oidc-state", &q.state) {
            return Err(IdentityError::Unauthenticated);
        }
        if f.try_get::<String, _>("phase")? == "retired" {
            return Err(IdentityError::Conflict);
        }
        if f.try_get::<String, _>("phase")? == "denied" {
            return Err(IdentityError::Forbidden);
        }
        let p = self
            .current_oidc_flow(&mut tx, &f, f.try_get::<String, _>("phase")? == "committed")
            .await?;
        if f.try_get::<String, _>("phase")? == "committed" {
            let code_sha: String = sqlx::query_scalar(
                "SELECT code_sha256 FROM android_oidc_exchanges WHERE flow_id=$1",
            )
            .bind(q.flow_id.to_string())
            .fetch_one(&mut *tx)
            .await?;
            if code_sha != secret_hash("oidc-code", &q.code) {
                return Err(IdentityError::Conflict);
            }
            let original = self
                .original_oidc_receipt(&mut tx, &f, keys.as_ref())
                .await?;
            tx.commit().await?;
            return Ok(original);
        }
        let payload: FlowPayload = serde_json::from_slice(
            &cipher::read(
                &mut tx,
                keys.as_ref(),
                identifier(f.try_get("payload_id")?)?,
                p.id,
                "oauth_flow",
            )
            .await?,
        )
        .map_err(|_| IdentityError::Corrupt)?;
        let code_sha = secret_hash("oidc-code", &q.code);
        let mut dispatch = false;
        let nonce;
        if let Some(exchange) = sqlx::query("SELECT * FROM android_oidc_exchanges WHERE flow_id=$1")
            .bind(q.flow_id.to_string())
            .fetch_optional(&mut *tx)
            .await?
        {
            if exchange.try_get::<String, _>("code_sha256")? != code_sha {
                return Err(IdentityError::Conflict);
            }
            nonce = exchange.try_get::<String, _>("dispatch_nonce")?;
            if exchange
                .try_get::<Option<String>, _>("response_id")?
                .is_none()
            {
                return Err(IdentityError::Conflict);
            }
        } else {
            nonce = random();
            let at = self.sample(&mut tx).await?;
            sqlx::query("INSERT INTO android_oidc_exchanges(flow_id,dispatch_nonce,code_sha256,request_sha256,state,dispatched_at) VALUES($1,$2,$3,$4,'dispatched',$5)").bind(q.flow_id.to_string()).bind(&nonce).bind(&code_sha).bind(mac(keys.as_ref(),key,&json!({"flow":q.flow_id,"code":q.code,"redirect":payload.redirect_uri,"pkce":payload.pkce_verifier}))?).bind(at).execute(&mut *tx).await?;
            sqlx::query(
                "UPDATE android_oidc_flows SET phase='dispatched' WHERE id=$1 AND phase='prepared'",
            )
            .bind(q.flow_id.to_string())
            .execute(&mut *tx)
            .await?;
            dispatch = true;
        }
        self.current_oidc_flow(&mut tx, &f, false).await?;
        let secret_id: Option<String> =
            sqlx::query_scalar("SELECT secret_id FROM android_oidc_profiles WHERE id=$1")
                .bind(p.id.to_string())
                .fetch_one(&mut *tx)
                .await?;
        let client_secret = match secret_id {
            Some(id) => Some(Zeroizing::new(
                String::from_utf8(
                    cipher::read(
                        &mut tx,
                        keys.as_ref(),
                        identifier(id)?,
                        p.id,
                        "oauth_profile",
                    )
                    .await?
                    .to_vec(),
                )
                .map_err(|_| IdentityError::Corrupt)?,
            )),
            None => None,
        };
        self.current_oidc_flow(&mut tx, &f, false).await?;
        tx.commit().await?;
        if dispatch {
            *producer.lock().map_err(|_| IdentityError::Corrupt)? = Some(nonce.clone());
            let exchange_http = CapturingHttp {
                store: self.clone(),
                profile: p.clone(),
                flow_id: q.flow_id,
                nonce: nonce.clone(),
                keys: keys.clone(),
                key,
            };
            let client = CoreClient::from_provider_metadata(
                payload.metadata.clone(),
                ClientId::new(p.config.client_id.clone()),
                client_secret
                    .as_ref()
                    .map(|v| ClientSecret::new(v.to_string())),
            )
            .set_redirect_uri(
                RedirectUrl::new(payload.redirect_uri.clone())
                    .map_err(|_| IdentityError::Corrupt)?,
            );
            let result = client
                .exchange_code(AuthorizationCode::new(q.code.clone()))
                .map_err(|_| IdentityError::Unauthenticated)?
                .set_pkce_verifier(PkceCodeVerifier::new(payload.pkce_verifier.clone()))
                .request_async(&exchange_http)
                .await;
            if result.is_err() {
                return Err(IdentityError::Unauthenticated);
            }
        }
        let mut tx = self.begin().await?;
        let f = flow(&mut tx, q.flow_id, &q.resume_secret).await?;
        if f.try_get::<String, _>("phase")? == "retired" {
            return Err(IdentityError::Conflict);
        }
        if f.try_get::<String, _>("phase")? == "committed" {
            self.current_oidc_flow(&mut tx, &f, true).await?;
            let original = self
                .original_oidc_receipt(&mut tx, &f, keys.as_ref())
                .await?;
            tx.commit().await?;
            return Ok(original);
        }
        let p = self.current_oidc_flow(&mut tx, &f, false).await?;
        let exchange=sqlx::query("SELECT * FROM android_oidc_exchanges WHERE flow_id=$1 AND dispatch_nonce=$2 AND code_sha256=$3 AND state='received'").bind(q.flow_id.to_string()).bind(&nonce).bind(code_sha).fetch_one(&mut *tx).await?;
        if exchange.try_get::<i64, _>("response_status")? != 200 {
            return Err(IdentityError::Unauthenticated);
        }
        let original = cipher::read(
            &mut tx,
            keys.as_ref(),
            identifier(
                exchange
                    .try_get::<Option<String>, _>("response_id")?
                    .ok_or(IdentityError::Corrupt)?,
            )?,
            p.id,
            "oauth_response",
        )
        .await?;
        if digest(&original)
            != exchange
                .try_get::<Option<String>, _>("response_sha256")?
                .ok_or(IdentityError::Corrupt)?
        {
            return Err(IdentityError::Corrupt);
        }
        let http_original: OriginalHttp =
            serde_json::from_slice(&original).map_err(|_| IdentityError::Corrupt)?;
        if http_original.status != 200
            || !http_original.headers.iter().any(|(name, value)| {
                name == "content-type"
                    && STANDARD
                        .decode(value)
                        .ok()
                        .and_then(|v| String::from_utf8(v).ok())
                        .is_some_and(|v| v.to_ascii_lowercase().starts_with("application/json"))
            })
        {
            return Err(IdentityError::Unauthenticated);
        }
        let body = Zeroizing::new(
            STANDARD
                .decode(&http_original.body_base64)
                .map_err(|_| IdentityError::Corrupt)?,
        );
        let response: CoreTokenResponse =
            serde_json::from_slice(&body).map_err(|_| IdentityError::Unauthenticated)?;
        let now = self.sample(&mut tx).await?;
        let subject = verify(
            VerificationContext {
                metadata: &payload.metadata,
                issuer: &p.config.issuer,
                client_id: &p.config.client_id,
                nonce: &payload.nonce,
                groups_claim: &p.config.groups_claim,
                flow_started_at: f.try_get("created_at")?,
                current: now,
            },
            &response,
        )?;
        self.current_oidc_flow(&mut tx, &f, false).await?;
        let previous=sqlx::query("SELECT * FROM android_oidc_external_identities WHERE profile_id=$1 AND issuer=$2 AND subject=$3").bind(p.id.to_string()).bind(&subject.issuer).bind(&subject.subject).fetch_optional(&mut *tx).await?;
        let linked = f.try_get::<Option<String>, _>("link_owner_id")?;
        let (external, owner, external_rev) = if let Some(old) = previous {
            if linked
                .as_ref()
                .is_some_and(|v| Some(v) != old.try_get::<String, _>("identity_id").ok().as_ref())
            {
                return Err(IdentityError::Forbidden);
            }
            let old_revision: i64 = old.try_get("revision")?;
            let revision = if old.try_get::<i64, _>("active")? != 1 {
                if linked.as_ref() != Some(&old.try_get::<String, _>("identity_id")?) {
                    return Err(IdentityError::Forbidden);
                }
                let next = old_revision.checked_add(1).ok_or(IdentityError::Corrupt)?;
                if sqlx::query("UPDATE android_oidc_external_identities SET active=1,revision=$1 WHERE id=$2 AND revision=$3 AND active=0")
                    .bind(next).bind(old.try_get::<String, _>("id")?).bind(old_revision).execute(&mut *tx).await?.rows_affected() != 1 { return Err(IdentityError::Conflict); }
                next
            } else {
                old_revision
            };
            (
                identifier(old.try_get("id")?)?,
                identifier(old.try_get("identity_id")?)?,
                revision,
            )
        } else {
            let owner = if let Some(linked) = linked {
                identifier(linked)?
            } else {
                if !p.config.allow_signup
                    || sso::admission(&mut tx, &p)
                        .await?
                        .is_some_and(|policy| !policy.provision)
                {
                    return Err(IdentityError::Forbidden);
                }
                let generated = random();
                let identity = self
                    .insert_identity(
                        &mut tx,
                        &format!("oidc_{}", Uuid::new_v4().simple()),
                        "OIDC account",
                        &generated,
                        false,
                    )
                    .await?;
                identity.id
            };
            let external = Uuid::new_v4();
            sqlx::query("INSERT INTO android_oidc_external_identities(id,profile_id,issuer,subject,identity_id,revision,active,created_at) VALUES($1,$2,$3,$4,$5,1,1,$6)").bind(external.to_string()).bind(p.id.to_string()).bind(&subject.issuer).bind(&subject.subject).bind(owner.to_string()).bind(now).execute(&mut *tx).await?;
            (external, owner, 1)
        };
        if !sso::apply(
            self,
            &mut tx,
            &p,
            &f,
            owner,
            external,
            external_rev,
            &subject,
        )
        .await?
        {
            let denial = json!({"flow_id":q.flow_id,"denied":"signed_groups_or_auto_provision_policy","claims_sha256":subject.claims_sha256,"groups_sha256":digest(encoded(&subject.groups)?.as_bytes()),"session_issued":false,"external_tokens_revoked":false});
            let at = self.sample(&mut tx).await?;
            let record = cipher::put(
                &mut tx,
                keys.as_ref(),
                key,
                &p,
                Uuid::new_v4(),
                "oauth_response",
                &serde_json::to_vec(&denial).map_err(|_| IdentityError::Corrupt)?,
                at,
            )
            .await?;
            self.current_oidc_flow(&mut tx, &f, false).await?;
            if sqlx::query(
                "UPDATE android_oidc_flows SET phase='denied' WHERE id=$1 AND phase='received'",
            )
            .bind(q.flow_id.to_string())
            .execute(&mut *tx)
            .await?
            .rows_affected()
                != 1
            {
                return Err(IdentityError::Conflict);
            }
            sqlx::query(
                "INSERT INTO android_oidc_denials(flow_id,receipt_id,created_at) VALUES($1,$2,$3)",
            )
            .bind(q.flow_id.to_string())
            .bind(record.to_string())
            .bind(at)
            .execute(&mut *tx)
            .await?;
            self.audit_event(
                &mut tx,
                p.organization_id,
                owner,
                "oidc_signed_group_admission_denied",
                q.flow_id,
            )
            .await?;
            tx.commit().await?;
            return Err(IdentityError::Forbidden);
        }
        let identity_row = sqlx::query("SELECT * FROM identity_subjects WHERE id=$1 AND active=1")
            .bind(owner.to_string())
            .fetch_one(&mut *tx)
            .await?;
        let identity = identity(&identity_row)?;
        if let Some(org) = p.organization_id {
            let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM identity_memberships WHERE identity_id=$1 AND organization_id=$2 AND active=1").bind(owner.to_string()).bind(org.to_string()).fetch_one(&mut *tx).await?;
            if n != 1 {
                return Err(IdentityError::Forbidden);
            }
        }
        let at = self.sample(&mut tx).await?;
        let expiry = subject.expires_at.min(at + 3600000);
        if expiry < at + 1000 {
            return Err(IdentityError::Unauthenticated);
        }
        let issued = self.session(&mut tx, identity, expiry - at).await?;
        sqlx::query("UPDATE identity_sessions SET expires_at=CASE WHEN expires_at>$1 THEN $1 ELSE expires_at END WHERE identity_id=$2 AND token_sha256=$3 AND revoked=0").bind(expiry).bind(owner.to_string()).bind(secret_hash("session-v1",&issued.token)).execute(&mut *tx).await?;
        let sid = self
            .record_android_session_in_transaction(&mut tx, &issued, &payload.begin.device_id, None)
            .await?;
        if let Some(org) = p.organization_id {
            self.bind_session_scope(&mut tx, &issued, org, 15).await?;
        }
        sqlx::query("INSERT INTO android_oidc_session_origins(session_id,identity_id,profile_id,external_identity_id,external_revision,profile_revision,profile_epoch,config_sha256,organization_epoch,source_grant_revision,flow_id,claims_sha256,verified_at,authentication_time,expires_at,sso_revision,sso_epoch) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$6,$10,$11,$12,$13,$14,$15,$16)").bind(sid.to_string()).bind(owner.to_string()).bind(p.id.to_string()).bind(external.to_string()).bind(external_rev).bind(p.revision).bind(p.epoch.to_string()).bind(&p.config_sha256).bind(f.try_get::<i64,_>("organization_epoch")?).bind(q.flow_id.to_string()).bind(&subject.claims_sha256).bind(at).bind(subject.authentication_time.unwrap_or(0)).bind(subject.expires_at).bind(f.try_get::<i64,_>("sso_revision")?).bind(f.try_get::<Option<String>,_>("sso_epoch")?).execute(&mut *tx).await?;
        let inner = ReceiptBytes {
            access_token: issued.token,
            user_id: owner,
            session_id: sid,
            expires_at: expiry,
            flow_id: q.flow_id,
            claims_sha256: subject.claims_sha256,
            linked: payload.begin.purpose == "link",
        };
        let bytes = Zeroizing::new(serde_json::to_vec(&inner).map_err(|_| IdentityError::Corrupt)?);
        let payload_id = cipher::put(
            &mut tx,
            keys.as_ref(),
            key,
            &p,
            Uuid::new_v4(),
            "oauth_token",
            &bytes,
            at,
        )
        .await?;
        sqlx::query("INSERT INTO android_oidc_receipts(flow_id,identity_id,session_id,external_identity_id,payload_id,receipt_sha256,committed_at) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(q.flow_id.to_string()).bind(owner.to_string()).bind(sid.to_string()).bind(external.to_string()).bind(payload_id.to_string()).bind(digest(&bytes)).bind(at).execute(&mut *tx).await?;
        if sqlx::query(
            "UPDATE android_oidc_flows SET phase='committed' WHERE id=$1 AND phase='received'",
        )
        .bind(q.flow_id.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            != 1
        {
            return Err(IdentityError::Conflict);
        }
        self.current_oidc_flow(&mut tx, &f, false).await?;
        let actor = self
            .authenticate_android_token_in_transaction(&mut tx, &inner.access_token)
            .await?;
        self.validate_oidc_origin_in_transaction(&mut tx, &actor)
            .await?;
        self.audit_event(
            &mut tx,
            p.organization_id,
            owner,
            "oidc_verified_session",
            sid,
        )
        .await?;
        tx.commit().await?;
        Ok(OidcSessionReceipt { inner })
    }
}
fn view(row: &sqlx::any::AnyRow, p: &FlowPayload) -> Result<AndroidOidcFlow> {
    Ok(AndroidOidcFlow {
        id: identifier(row.try_get("id")?)?,
        profile_id: identifier(row.try_get("profile_id")?)?,
        profile_revision: row.try_get("profile_revision")?,
        purpose: row.try_get("purpose")?,
        authorization_url: p.authorization_url.clone(),
        expires_at: row.try_get("expires_at")?,
        phase: row.try_get("phase")?,
    })
}
