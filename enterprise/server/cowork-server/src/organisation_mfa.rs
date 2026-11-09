//! Organisation MFA is verified against the current session and scope. Neither
//! factor enrolment nor a client assertion is evidence of a verified sign-in.
use crate::{auth::Principal, error::ApiError, AppState};
use axum::http::Method;
use chrono::{DateTime, Duration, Utc};
use cowork_contracts::organisation_mfa::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use std::{future::Future, sync::OnceLock};
use uuid::Uuid;

#[derive(Clone)]
pub struct RequestAuthority {
    pub user: Uuid,
    pub session: Uuid,
    pub token_hash: Option<Vec<u8>>,
}
tokio::task_local! {static REQUEST_AUTHORITY:RequestAuthority;}
pub async fn scope<T>(authority: RequestAuthority, work: impl Future<Output = T>) -> T {
    REQUEST_AUTHORITY.scope(authority, work).await
}
pub async fn optional_scope<T>(
    user: Uuid,
    session: Option<Uuid>,
    work: impl Future<Output = T>,
) -> T {
    if let Some(session) = session {
        scope(
            RequestAuthority {
                user,
                session,
                token_hash: None,
            },
            work,
        )
        .await
    } else {
        work.await
    }
}
pub async fn guard_pool<T>(
    pool: &PgPool,
    user: Uuid,
    session: Option<Uuid>,
    work: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    session_pool(pool, user, session)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    tokio::pin!(work);
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {biased;_=interval.tick()=>session_pool(pool,user,session).await.map_err(|e|anyhow::anyhow!("{e}"))?,result=&mut work=>return result}
    }
}
pub fn request_authority(user: Uuid) -> Option<RequestAuthority> {
    REQUEST_AUTHORITY
        .try_with(|a| a.clone())
        .ok()
        .filter(|a| a.user == user)
}
static INSTANCE: OnceLock<Option<InstanceMfaConfiguration>> = OnceLock::new();
pub fn configuration() -> anyhow::Result<Option<InstanceMfaConfiguration>> {
    let value = match std::env::var("COWORK_ORGANISATION_MFA_POLICY") {
        Ok(s) => Some(serde_json::from_str::<InstanceMfaConfiguration>(&s)?),
        Err(std::env::VarError::NotPresent) => None,
        Err(e) => return Err(e.into()),
    };
    if let Some(v) = &value {
        v.policy.validate().map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            v.version == 1 && !v.instance_id.is_nil(),
            "unsupported instance MFA configuration"
        );
        anyhow::ensure!(
            std::env::var("COWORK_ORGANISATION_ID")?.parse::<Uuid>()? == v.policy.organisation_id
                && std::env::var("COWORK_INSTANCE_ID")?.parse::<Uuid>()? == v.instance_id,
            "MFA configuration belongs to a different instance or organisation"
        );
        anyhow::ensure!(
            crate::fleet::enabled(),
            "organisation MFA requires current management authority"
        );
        crate::management_transport::client(
            &std::env::var("COWORK_FLEET_MANAGEMENT_URL")?,
            std::time::Duration::from_secs(5),
        )?;
    }
    Ok(value)
}
pub fn initialize() -> anyhow::Result<()> {
    INSTANCE
        .set(configuration()?)
        .map_err(|_| anyhow::anyhow!("organisation MFA already initialized"))
}
pub fn instance_contract() -> Option<u8> {
    instance().map(|_| 1)
}
pub async fn fleet_capability(pool: &PgPool) -> Result<Option<u8>, ApiError> {
    if instance_contract() != Some(1) {
        return Ok(None);
    }
    // API presence alone does not imply that a separate worker enforces MFA.
    // Old/missing worker observations remain explicit incompatible evidence.
    let supported:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_process_health WHERE process_mode IN ('worker','all') AND lifecycle='running' AND last_seen_at>clock_timestamp()-interval '20 seconds' AND observations->>'organisation_mfa_version'='1') AND NOT EXISTS(SELECT 1 FROM server_process_health WHERE process_mode IN ('worker','all') AND lifecycle<>'stopped' AND last_seen_at>clock_timestamp()-interval '20 seconds' AND coalesce(observations->>'organisation_mfa_version','0')<>'1')")
        .fetch_one(pool).await?;
    Ok(supported.then_some(1))
}
fn instance() -> Option<&'static InstanceMfaConfiguration> {
    INSTANCE.get().and_then(Option::as_ref)
}
fn denied() -> ApiError {
    ApiError::MfaRequired("verify a current second factor for this organisation and session".into())
}
fn unavailable() -> ApiError {
    ApiError::Unavailable("current organisation MFA authority is unavailable".into())
}
pub fn organisation_from_path(path: &str) -> Option<Uuid> {
    path.split('/')
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|w| {
            (w[0] == "organisations")
                .then(|| w[1].parse().ok())
                .flatten()
        })
}
pub fn recovery_route(method: &Method, path: &str) -> bool {
    if let Some(tail) = path.strip_prefix("/api/v1/runs/") {
        let p: Vec<_> = tail.split('/').collect();
        if p.first().is_some_and(|s| s.parse::<Uuid>().is_ok())
            && ((method == Method::POST && p.len() == 2 && p[1] == "cancel")
                || (method == Method::DELETE
                    && p.len() == 3
                    && p[1] == "desktop-sessions"
                    && p[2].parse::<Uuid>().is_ok()))
        {
            return true;
        }
    }
    matches!(
        (method.as_str(), path),
        (
            "GET",
            "/api/v1/auth/assurance"
                | "/api/v1/auth/totp"
                | "/api/v1/auth/me"
                | "/api/v1/version"
                | "/api/v1/runtime"
        ) | (
            "POST",
            "/api/v1/auth/assurance/verify"
                | "/api/v1/auth/logout"
                | "/api/v1/auth/totp/setup"
                | "/api/v1/auth/totp/enable"
                | "/api/v1/auth/passkeys/register/start"
                | "/api/v1/auth/passkeys/register/finish"
        )
    )
}
pub async fn session_in_tx(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    session: Uuid,
    exclusive: bool,
) -> Result<PgRow, ApiError> {
    let original:Option<Value>=sqlx::query_scalar("SELECT auth_origin FROM auth_sessions WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL AND expires_at>clock_timestamp()").bind(session).bind(user).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::Unauthorized("current user session is unavailable".into()))?;
    crate::management::directory::validate_origin(state, tx, user, original.clone()).await?;
    let sql = if exclusive {
        "SELECT s.*,u.platform_admin,u.deleted_at,clock_timestamp() AS checked_at FROM auth_sessions s JOIN users u ON u.id=s.user_id WHERE s.id=$1 AND s.user_id=$2 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND u.deleted_at IS NULL FOR UPDATE OF s FOR SHARE OF u"
    } else {
        "SELECT s.*,u.platform_admin,u.deleted_at,clock_timestamp() AS checked_at FROM auth_sessions s JOIN users u ON u.id=s.user_id WHERE s.id=$1 AND s.user_id=$2 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND u.deleted_at IS NULL FOR SHARE OF s,u"
    };
    let row = sqlx::query(sql)
        .bind(session)
        .bind(user)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("current user session is unavailable".into()))?;
    if row.get::<Option<Value>, _>("auth_origin") != original {
        return Err(ApiError::Unauthorized(
            "original session origin changed while waiting".into(),
        ));
    }
    // Recheck wall time after provider/directory SQL lock waits.
    let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth_sessions s JOIN users u ON u.id=s.user_id WHERE s.id=$1 AND s.user_id=$2 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND u.deleted_at IS NULL)").bind(session).bind(user).fetch_one(&mut **tx).await?;
    if !active {
        return Err(ApiError::Unauthorized(
            "current user session expired while waiting".into(),
        ));
    }
    Ok(row)
}
pub async fn assured_in_tx(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    session: &PgRow,
    policy: &OrganisationMfaPolicy,
) -> Result<(bool, Option<DateTime<Utc>>), ApiError> {
    let now = Utc::now();
    let verified: Option<DateTime<Utc>> = session.get("second_factor_verified_at");
    let factors:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_totp WHERE user_id=$1 AND enabled) OR EXISTS(SELECT 1 FROM passkeys WHERE user_id=$1)").bind(user).fetch_one(&mut **tx).await?;
    if factors {
        if let Some(at) = verified
            .filter(|at| *at <= now && *at + Duration::seconds(policy.max_age_seconds.into()) > now)
        {
            return Ok((
                true,
                Some(at + Duration::seconds(policy.max_age_seconds.into())),
            ));
        }
    }
    if policy.allow_validated_idp {
        if let Some(e) = crate::oidc_assurance::evidence(session.get("oidc_assurance")) {
            if e.organisation
                .as_ref()
                .is_none_or(|o| o.id == policy.organisation_id)
                && e.authenticated_at <= now
                && e.authenticated_at + Duration::seconds(policy.max_age_seconds.into()) > now
                && crate::oidc_assurance::valid_in_tx(state, tx, user, &e).await?
            {
                let until = e
                    .valid_until
                    .min(e.authenticated_at + Duration::seconds(policy.max_age_seconds.into()));
                if until > Utc::now() {
                    return Ok((true, Some(until)));
                }
            }
        }
    }
    Ok((false, None))
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityRequest {
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub user_id: Uuid,
    pub management_session_id: Option<Uuid>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityResponse {
    pub policy: OrganisationMfaPolicy,
    pub management_session_id: Option<Uuid>,
    pub user_id: Uuid,
    pub instance_id: Uuid,
    pub verified: bool,
    pub verified_until: Option<DateTime<Utc>>,
}
pub async fn remote(
    user: Uuid,
    management_session_id: Option<Uuid>,
) -> Result<Option<AuthorityResponse>, ApiError> {
    if !crate::fleet::enabled() {
        return Ok(None);
    }
    let org: Uuid = std::env::var("COWORK_ORGANISATION_ID")
        .map_err(|_| unavailable())?
        .parse()
        .map_err(|_| unavailable())?;
    let id: Uuid = std::env::var("COWORK_INSTANCE_ID")
        .map_err(|_| unavailable())?
        .parse()
        .map_err(|_| unavailable())?;
    let base = std::env::var("COWORK_FLEET_MANAGEMENT_URL").map_err(|_| unavailable())?;
    let client = crate::management_transport::client(&base, std::time::Duration::from_secs(5))
        .map_err(|_| unavailable())?;
    let mut response = client
        .post(format!(
            "{}/management/runner/v1/mfa-authority",
            base.trim_end_matches('/')
        ))
        .header(
            "x-cowork-instance-key",
            crate::fleet::secret("COWORK_FLEET_KEY_FILE")?,
        )
        .json(&AuthorityRequest {
            organisation_id: org,
            instance_id: id,
            user_id: user,
            management_session_id,
        })
        .send()
        .await
        .map_err(|_| unavailable())?;
    if !response.status().is_success() {
        return Err(unavailable());
    }
    if response.content_length().is_some_and(|n| n > 16384) {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len().saturating_add(chunk.len()) > 16384 {
            return Err(unavailable());
        }
        bytes.extend_from_slice(&chunk);
    }
    let r: AuthorityResponse = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    r.policy.validate().map_err(|_| unavailable())?;
    if r.policy.organisation_id != org
        || r.instance_id != id
        || r.user_id != user
        || r.management_session_id != management_session_id
        || r.verified && !r.verified_until.is_some_and(|t| t > Utc::now())
    {
        return Err(unavailable());
    }
    if let Some(config) = instance() {
        if r.policy.revision < config.policy.revision {
            return Err(unavailable());
        }
    }
    // An old, unqualified instance is never silently upgraded to MFA support.
    if r.policy.required && instance().is_none() {
        return Err(unavailable());
    }
    Ok(Some(r))
}
fn launch_session(row: &PgRow) -> Result<Option<Uuid>, ApiError> {
    let origin: Option<Value> = row.get("auth_origin");
    let Some(o) = origin else { return Ok(None) };
    if o["kind"] != "instance_launch" {
        return Ok(None);
    }
    match o.get("management_session_id") {
        Some(Value::String(s)) => Ok(Some(s.parse().map_err(|_| denied())?)),
        None | Some(Value::Null) => Ok(None),
        _ => Err(denied()),
    }
}
fn local_origin(origin: &Option<Value>, r: &AuthorityResponse) -> bool {
    origin.as_ref().is_some_and(|o| {
        o["version"] == 1
            && (matches!(o["kind"].as_str(), Some("local" | "oidc"))
                || (o["kind"] == "instance_launch"
                    && o["organisation_id"] == json!(r.policy.organisation_id)
                    && o["instance_id"] == json!(r.instance_id)
                    && r.management_session_id.is_some()
                    && o["management_session_id"] == json!(r.management_session_id)))
    })
}
pub async fn current_policy(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    row: &PgRow,
    org: Option<Uuid>,
) -> Result<(Option<OrganisationMfaPolicy>, bool, Option<DateTime<Utc>>), ApiError> {
    if state.management_mode {
        let Some(org) = org else {
            return Ok((None, false, None));
        };
        crate::management::organisation_lifecycle::share_active(tx, org).await?;
        let member:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_members WHERE organisation_id=$1 AND user_id=$2 AND role IN ('admin','member','auditor')) OR EXISTS(SELECT 1 FROM users WHERE id=$2 AND platform_admin AND deleted_at IS NULL)").bind(org).bind(user).fetch_one(&mut **tx).await?;
        if !member {
            return Err(ApiError::NotFound("organisation is not accessible".into()));
        }
        let p = crate::management::mfa::policy(tx, org).await?;
        let (v, t) = assured_in_tx(state, tx, user, row, &p).await?;
        return Ok((Some(p), v, t));
    }
    if let Some(r) = remote(user, launch_session(row)?).await? {
        let (local, until) = assured_in_tx(state, tx, user, row, &r.policy).await?;
        // Re-read the original local session after the external policy wait.
        let sid: Uuid = row.get("id");
        let live=sqlx::query("SELECT id,auth_origin,second_factor_verified_at,oidc_assurance FROM auth_sessions WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR SHARE").bind(sid).bind(user).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
        if live.get::<Option<Value>, _>("auth_origin") != row.get::<Option<Value>, _>("auth_origin")
            || live.get::<Option<DateTime<Utc>>, _>("second_factor_verified_at")
                != row.get::<Option<DateTime<Utc>>, _>("second_factor_verified_at")
            || live.get::<Option<Value>, _>("oidc_assurance")
                != row.get::<Option<Value>, _>("oidc_assurance")
        {
            return Err(denied());
        }
        let origin: Option<Value> = row.get("auth_origin");
        let original_local = local_origin(&origin, &r);
        let central = r.verified && launch_session(row)?.is_some();
        return Ok((
            Some(r.policy),
            central || (local && original_local),
            if central { r.verified_until } else { until },
        ));
    }
    Ok((None, false, None))
}
pub struct RequestGuard {
    pub verified_until: DateTime<Utc>,
    tx: Transaction<'static, Postgres>,
}
impl RequestGuard {
    pub async fn finish(self) -> Result<(), ApiError> {
        if self.verified_until <= Utc::now() {
            return Err(denied());
        }
        self.tx.commit().await?;
        Ok(())
    }
}
/// Includes streamed events and downloads: a completed HTTP handshake is not
/// permission to keep reading after expiry, logout or a policy tightening.
pub fn protect_response(
    state: AppState,
    authority: RequestAuthority,
    org: Option<Uuid>,
    response: axum::response::Response,
) -> axum::response::Response {
    use futures_util::StreamExt;
    if response.status() == axum::http::StatusCode::UNAUTHORIZED {
        // The request was already denied. Never stream the original body after
        // revocation, but deliver a fixed rejection instead of a truncated JSON
        // response. Successful/private bodies still require every-frame checks.
        use axum::response::IntoResponse;
        return ApiError::Unauthorized("authentication is no longer valid".into()).into_response();
    }
    let (parts, body) = response.into_parts();
    let stream = async_stream::stream! {
        let mut data=body.into_data_stream();
        while let Some(item)=data.next().await {
            let checked=async {
                let mut tx=state.pool.begin().await?;
                let row=session_in_tx(&state,&mut tx,authority.user,authority.session,false).await?;
                if let Some(hash)=&authority.token_hash {let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM access_tokens WHERE token_hash=$1 AND session_id=$2 AND user_id=$3 AND revoked_at IS NULL AND expires_at>clock_timestamp())").bind(hash).bind(authority.session).bind(authority.user).fetch_one(&mut *tx).await?;if !active{return Err(denied());}}
                let (policy,verified,_)=current_policy(&state,&mut tx,authority.user,&row,org).await?;
                if policy.is_some_and(|p|p.required)&&!verified{return Err(denied());}
                tx.commit().await?;Ok::<_,ApiError>(())
            }.await;
            if let Err(e)=checked {yield Err::<axum::body::Bytes,std::io::Error>(std::io::Error::other(e.to_string()));break;}
            match item {Ok(bytes)=>yield Ok(bytes),Err(e)=>{yield Err(std::io::Error::other(e.to_string()));break;}}
        }
    };
    axum::response::Response::from_parts(parts, axum::body::Body::from_stream(stream))
}
/// Session and policy locks survive downstream SQL waits. Security mutations
/// instead recheck under their own transaction, avoiding self-lock upgrades.
pub async fn request_guard(
    state: &AppState,
    principal: &Principal,
    token_hash: &[u8],
    method: &Method,
    path: &str,
) -> Result<Option<RequestGuard>, ApiError> {
    if principal.bootstrap {
        return Ok(None);
    }
    let session = principal.session_id.ok_or_else(denied)?;
    if recovery_route(method, path) {
        return Ok(None);
    }
    let mut tx = state.pool.begin().await?;
    let policy_edit = path.ends_with("/mfa-policy");
    let security = path.starts_with("/api/v1/auth/") || policy_edit;
    let row = session_in_tx(state, &mut tx, principal.user_id, session, false).await?;
    let (policy, verified, until) = current_policy(
        state,
        &mut tx,
        principal.user_id,
        &row,
        organisation_from_path(path),
    )
    .await?;
    if policy.as_ref().is_some_and(|p| p.required) && !verified && !recovery_route(method, path) {
        return Err(denied());
    }
    let original_origin: Option<Value> = row.get("auth_origin");
    let original_factor: Option<DateTime<Utc>> = row.get("second_factor_verified_at");
    let original_idp: Option<Value> = row.get("oidc_assurance");
    tx.commit().await?;
    if security || !policy.as_ref().is_some_and(|p| p.required) {
        return Ok(None);
    }
    // Provider/directory validation owns org locks. Release those before a
    // downstream admin handler takes the same org lock; hold the independent
    // MFA slot and original session through the downstream transaction instead.
    let mut tx = state.pool.begin().await?;
    if state.management_mode {
        let current = crate::management::mfa::policy(
            &mut tx,
            policy.as_ref().ok_or_else(denied)?.organisation_id,
        )
        .await?;
        if Some(&current) != policy.as_ref() {
            return Err(denied());
        }
    }
    let current=sqlx::query("SELECT s.auth_origin,s.second_factor_verified_at,s.oidc_assurance FROM auth_sessions s JOIN access_tokens a ON a.session_id=s.id AND a.user_id=s.user_id JOIN users u ON u.id=s.user_id WHERE s.id=$1 AND s.user_id=$2 AND a.token_hash=$3 AND a.revoked_at IS NULL AND a.expires_at>clock_timestamp() AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND u.deleted_at IS NULL FOR SHARE OF s,a")
        .bind(session).bind(principal.user_id).bind(token_hash).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    if current.get::<Option<Value>, _>("auth_origin") != original_origin
        || current.get::<Option<DateTime<Utc>>, _>("second_factor_verified_at") != original_factor
        || current.get::<Option<Value>, _>("oidc_assurance") != original_idp
        || until.is_none_or(|u| u <= Utc::now())
    {
        return Err(denied());
    }
    Ok(Some(RequestGuard {
        verified_until: until.ok_or_else(denied)?,
        tx,
    }))
}

pub async fn enrollment(state: &AppState, principal: &Principal) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await?;
    enrollment_tx(state, &mut tx, principal).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn enrollment_tx(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    principal: &Principal,
) -> Result<(), ApiError> {
    let sid = principal.session_id.ok_or_else(denied)?;
    let row = session_in_tx(state, tx, principal.user_id, sid, true).await?;
    let factors:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_totp WHERE user_id=$1 AND enabled) OR EXISTS(SELECT 1 FROM passkeys WHERE user_id=$1)").bind(principal.user_id).fetch_one(&mut **tx).await?;
    let created: DateTime<Utc> = row.get("created_at");
    let local_at: Option<DateTime<Utc>> = row.get("second_factor_verified_at");
    let local_verified = factors
        && local_at.is_some_and(|at| at <= Utc::now() && at + Duration::minutes(10) > Utc::now());
    let provider_verified =
        if let Some(e) = crate::oidc_assurance::evidence(row.get("oidc_assurance")) {
            e.authenticated_at <= Utc::now()
                && e.authenticated_at + Duration::minutes(10) > Utc::now()
                && crate::oidc_assurance::valid_in_tx(state, tx, principal.user_id, &e).await?
        } else {
            false
        };
    let verified = local_verified || provider_verified;
    let recent = created <= Utc::now() && created + Duration::minutes(10) > Utc::now();
    // A fresh primary login may install its very first local factor. Once a
    // factor exists, adding/replacing factors requires fresh step-up.
    if !verified && (factors || !recent) {
        return Err(denied());
    }
    Ok(())
}
pub async fn protect_last_factor(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    removing_totp: bool,
) -> Result<(), ApiError> {
    // Serialize activation with factor removal even while the policy is still
    // optional; otherwise activation can count a concurrently removed factor.
    let _orgs=sqlx::query("SELECT o.id FROM management_organisations o JOIN management_members m ON m.organisation_id=o.id WHERE m.user_id=$1 AND o.archived_at IS NULL AND m.role IN ('admin','member','auditor') ORDER BY o.id FOR SHARE OF o").bind(user).fetch_all(&mut **tx).await?;
    let required:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM organisation_mfa_policies p JOIN management_organisations o ON o.id=p.organisation_id AND o.archived_at IS NULL JOIN management_members m ON m.organisation_id=p.organisation_id WHERE m.user_id=$1 AND m.role IN ('admin','member','auditor') AND p.required)").bind(user).fetch_one(&mut **tx).await?;
    let instance_required = remote(user, None).await?.is_some_and(|r| r.policy.required);
    if !required && !instance_required {
        return Ok(());
    }
    let _: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut **tx)
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM passkeys WHERE user_id=$1")
        .bind(user)
        .fetch_one(&mut **tx)
        .await?;
    let totp: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_totp WHERE user_id=$1 AND enabled)")
            .bind(user)
            .fetch_one(&mut **tx)
            .await?;
    if (removing_totp && count < 1) || (!removing_totp && count < 2 && !totp) {
        return Err(ApiError::Conflict(
            "retain a local authenticator or passkey while organisation MFA is required".into(),
        ));
    }
    Ok(())
}
pub async fn bind_run(
    tx: &mut Transaction<'_, Postgres>,
    spec: &cowork_contracts::RunSpec,
) -> Result<(), ApiError> {
    let authority = request_authority(spec.creator_user_id);
    if let Some(a) = authority {
        session_tx(tx, a.user, Some(a.session)).await?;
        if let Some(hash) = &a.token_hash {
            let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM access_tokens WHERE token_hash=$1 AND session_id=$2 AND user_id=$3 AND revoked_at IS NULL AND expires_at>clock_timestamp())").bind(hash).bind(a.session).bind(a.user).fetch_one(&mut **tx).await?;
            if !valid {
                return Err(denied());
            }
        }
        sqlx::query("INSERT INTO run_mfa_admissions(run_id,owner_user_id,auth_session_id,spec_sha256) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(spec.id).bind(a.user).bind(a.session).bind(hex::encode(Sha256::digest(serde_json::to_vec(spec)?))).execute(&mut **tx).await?;
    } else if let Some(schedule) = spec
        .input
        .get("_schedule")
        .and_then(|v| v.get("schedule_id"))
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<Uuid>().ok())
    {
        sqlx::query("INSERT INTO run_mfa_admissions(run_id,owner_user_id,auth_session_id,spec_sha256) SELECT $1,a.owner_user_id,a.auth_session_id,$4 FROM schedule_mfa_admissions a JOIN schedules s ON s.id=a.schedule_id WHERE a.schedule_id=$2 AND a.owner_user_id=$3 AND s.created_by=$3 AND s.revision=a.revision ON CONFLICT DO NOTHING")
            .bind(spec.id).bind(schedule).bind(spec.creator_user_id).bind(hex::encode(Sha256::digest(serde_json::to_vec(spec)?))).execute(&mut **tx).await?;
    }
    Ok(())
}
pub async fn bind_schedule(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    user: Uuid,
    revision: i64,
) -> Result<(), ApiError> {
    if let Some(a) = request_authority(user) {
        session_tx(tx, user, Some(a.session)).await?;
        sqlx::query("INSERT INTO schedule_mfa_admissions(schedule_id,owner_user_id,auth_session_id,revision) VALUES($1,$2,$3,$4) ON CONFLICT(schedule_id) DO UPDATE SET auth_session_id=EXCLUDED.auth_session_id,revision=EXCLUDED.revision WHERE schedule_mfa_admissions.owner_user_id=EXCLUDED.owner_user_id")
        .bind(id).bind(user).bind(a.session).bind(revision).execute(&mut **tx).await?;
    }
    Ok(())
}
/// Workers and devices share this fence before claim and again after runner,
/// provider or SQL waits. Historical runs do not gain invented sessions.
pub async fn execution(
    tx: &mut Transaction<'_, Postgres>,
    run: &cowork_contracts::RunRecord,
) -> Result<(), ApiError> {
    if !crate::fleet::enabled() {
        return Ok(());
    }
    let row=sqlx::query("SELECT a.auth_session_id,a.spec_sha256,s.auth_origin,s.second_factor_verified_at,s.oidc_assurance,s.expires_at,s.revoked_at,u.deleted_at FROM (SELECT auth_session_id,spec_sha256,owner_user_id,0 AS queued_revision FROM run_mfa_admissions WHERE run_id=$1 UNION ALL SELECT c.auth_session_id,c.spec_sha256,c.owner_user_id,c.queued_revision FROM run_mfa_continuations c JOIN run_resume_requests r ON r.run_id=c.run_id AND r.request_id=c.request_id AND r.queued_revision=c.queued_revision AND r.user_id=c.owner_user_id WHERE c.run_id=$1 AND c.queued_revision<=$3) a JOIN auth_sessions s ON s.id=a.auth_session_id AND s.user_id=a.owner_user_id JOIN users u ON u.id=a.owner_user_id WHERE a.owner_user_id=$2 ORDER BY a.queued_revision DESC LIMIT 1")
        .bind(run.spec.id).bind(run.spec.creator_user_id).bind(run.revision).fetch_optional(&mut **tx).await?;
    let central = match &row {
        Some(r) => launch_session(r)?,
        None => None,
    };
    let Some(remote) = remote(run.spec.creator_user_id, central).await? else {
        return Ok(());
    };
    if !remote.policy.required {
        return Ok(());
    }
    let row = row.ok_or_else(denied)?;
    if row.get::<String, _>("spec_sha256")
        != hex::encode(Sha256::digest(serde_json::to_vec(&run.spec)?))
    {
        return Err(denied());
    }
    let sid: Uuid = row.get("auth_session_id");
    sql_execution_session(tx, run.spec.creator_user_id, sid, &remote).await
}
async fn sql_execution_session(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    sid: Uuid,
    remote: &AuthorityResponse,
) -> Result<(), ApiError> {
    let row=sqlx::query("SELECT s.*,clock_timestamp() AS checked_at FROM auth_sessions s JOIN users u ON u.id=s.user_id WHERE s.id=$1 AND s.user_id=$2 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND u.deleted_at IS NULL FOR SHARE OF s")
        .bind(sid).bind(user).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let origin: Option<Value> = row.get("auth_origin");
    if remote.verified
        && launch_session(&row)? == remote.management_session_id
        && remote.management_session_id.is_some()
        && remote.verified_until.is_some_and(|t| t > Utc::now())
    {
        return Ok(());
    }
    let local = local_origin(&origin, remote);
    if !local {
        return Err(denied());
    }
    let now: DateTime<Utc> = row.get("checked_at");
    let at: Option<DateTime<Utc>> = row.get("second_factor_verified_at");
    let factors:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_totp WHERE user_id=$1 AND enabled) OR EXISTS(SELECT 1 FROM passkeys WHERE user_id=$1)").bind(user).fetch_one(&mut **tx).await?;
    if factors
        && at.is_some_and(|a| {
            a <= now && a + Duration::seconds(remote.policy.max_age_seconds.into()) > Utc::now()
        })
    {
        return Ok(());
    }
    if remote.policy.allow_validated_idp {
        if let Some(e) = crate::oidc_assurance::evidence(row.get("oidc_assurance")) {
            let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oidc_assurance_policies p JOIN oidc_identities i ON i.id=$7 AND i.user_id=$8 AND i.issuer=p.issuer AND i.binding_method<>'legacy_unverified' WHERE p.scope=$1 AND p.active AND p.revision=$2 AND p.epoch=$3 AND p.fingerprint=$4 AND p.issuer=$5 AND p.client_id=$6)")
                .bind(&e.scope).bind(e.policy_revision).bind(e.policy_epoch).bind(&e.policy_fingerprint).bind(&e.issuer).bind(&e.client_id).bind(e.identity_id).bind(user).fetch_one(&mut **tx).await?;
            if e.version == 1
                && e.user_id == user
                && e.organisation.is_none()
                && valid
                && e.authenticated_at <= now
                && e.valid_until > Utc::now()
                && e.authenticated_at + Duration::seconds(remote.policy.max_age_seconds.into())
                    > Utc::now()
            {
                return Ok(());
            }
        }
    }
    Err(denied())
}
pub async fn execution_on(pool: &PgPool, run: Uuid) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    let r = crate::db::get_run(pool, run).await?;
    execution(&mut tx, &r).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn bind_resume(
    tx: &mut Transaction<'_, Postgres>,
    run: &cowork_contracts::RunRecord,
    session: Uuid,
    request: Uuid,
    revision: i64,
) -> Result<(), ApiError> {
    // Called only in the existing positive physical-stop + checkpoint resume
    // transaction. Original run admission and all old receipts stay intact.
    session_tx(tx, run.spec.creator_user_id, Some(session)).await?;
    sqlx::query("INSERT INTO run_mfa_continuations(run_id,queued_revision,owner_user_id,auth_session_id,request_id,spec_sha256) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(run.spec.id).bind(revision).bind(run.spec.creator_user_id).bind(session).bind(request).bind(hex::encode(Sha256::digest(serde_json::to_vec(&run.spec)?))).execute(&mut **tx).await?;
    Ok(())
}
pub async fn session_tx(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    sid: Option<Uuid>,
) -> Result<(), ApiError> {
    if !crate::fleet::enabled() {
        return Ok(());
    }
    let row = if let Some(sid) = sid {
        sqlx::query("SELECT auth_origin FROM auth_sessions WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR SHARE").bind(sid).bind(user).fetch_optional(&mut **tx).await?
    } else {
        None
    };
    if sid.is_some() && row.is_none() {
        return Err(denied());
    }
    let management_session_id = match row.as_ref() {
        Some(r) => launch_session(r)?,
        None => None,
    };
    let r = remote(user, management_session_id)
        .await?
        .ok_or_else(unavailable)?;
    if r.policy.required {
        sql_execution_session(tx, user, sid.ok_or_else(denied)?, &r).await?;
    }
    Ok(())
}
pub async fn session_pool(pool: &PgPool, user: Uuid, sid: Option<Uuid>) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    session_tx(&mut tx, user, sid).await?;
    tx.commit().await?;
    Ok(())
}
