//! Management administrator assurance is session-bound, never inferred from an
//! enrolled factor, refresh token, client flag or unvalidated identity-provider claim.
use crate::{
    auth::{self, Principal},
    error::ApiError,
    AppState,
};
use axum::{
    extract::{Extension, Query, State},
    http::Method,
    Json,
};
use cowork_contracts::{enterprise::AuthenticationAssurance, VerifyTotpRequest};
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

async fn read(
    state: &AppState,
    user: Uuid,
    session: Uuid,
    org: Option<Uuid>,
    step_up: bool,
) -> Result<AuthenticationAssurance, ApiError> {
    let row=sqlx::query("SELECT u.platform_admin,s.oidc_assurance,s.second_factor_verified_at, EXISTS(SELECT 1 FROM user_totp t WHERE t.user_id=u.id AND t.enabled) AS totp, EXISTS(SELECT 1 FROM passkeys p WHERE p.user_id=u.id) AS passkey, coalesce(s.second_factor_verified_at>now()-interval '12 hours' AND s.second_factor_verified_at<=now(),false) AS verified, s.created_at>now()-interval '10 minutes' AS recent FROM users u JOIN auth_sessions s ON s.user_id=u.id WHERE u.id=$1 AND s.id=$2 AND u.deleted_at IS NULL AND s.revoked_at IS NULL AND s.expires_at>now()")
        .bind(user).bind(session).fetch_optional(&state.pool).await?
        .ok_or_else(||ApiError::Unauthorized("session is unavailable".into()))?;
    let platform_required = state.management_mode && row.get::<bool, _>("platform_admin");
    let mut tx = state.pool.begin().await?;
    let current =
        crate::organisation_mfa::session_in_tx(state, &mut tx, user, session, false).await?;
    let (policy, mut organisation_verified, mut organisation_until) =
        crate::organisation_mfa::current_policy(state, &mut tx, user, &current, org).await?;
    if step_up {
        if let Some(mut p) = policy.clone() {
            p.max_age_seconds = p.max_age_seconds.min(600);
            (organisation_verified, organisation_until) =
                crate::organisation_mfa::assured_in_tx(state, &mut tx, user, &current, &p).await?;
        }
    }
    tx.commit().await?;
    let required = platform_required || policy.as_ref().is_some_and(|p| p.required);
    let totp = row.get::<bool, _>("totp");
    let passkey = row.get::<bool, _>("passkey");
    let original: Option<Value> = row.get("oidc_assurance");
    let provider = crate::oidc_assurance::session_evidence(state, user, original.clone()).await?;
    let (oidc_available, oidc_organisation_id) =
        crate::oidc_assurance::reauthentication(state, user, original).await?;
    let local_verified = if step_up {
        row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("second_factor_verified_at")
            .is_some_and(|t| {
                t <= chrono::Utc::now() && t + chrono::Duration::minutes(10) > chrono::Utc::now()
            })
    } else {
        row.get::<bool, _>("verified")
    };
    let provider_verified = provider.as_ref().is_some_and(|e| {
        !step_up || e.authenticated_at + chrono::Duration::minutes(10) > chrono::Utc::now()
    });
    let platform_verified = (local_verified && (totp || passkey)) || provider_verified;
    let organisation_required = policy.as_ref().is_some_and(|p| p.required);
    let verified = if platform_required && organisation_required {
        platform_verified && organisation_verified
    } else if organisation_required {
        organisation_verified
    } else {
        platform_verified || organisation_verified
    };
    let enrollment_allowed =
        !required || verified || (!totp && !passkey && row.get::<bool, _>("recent"));
    Ok(AuthenticationAssurance {
        required,
        totp_enabled: totp,
        passkey_registered: passkey,
        session_verified: verified,
        enrollment_allowed,
        reauthentication_required: required && !totp && !passkey && !enrollment_allowed,
        provider_verified: Some(provider.is_some()),
        provider_verification_expires_at: provider.as_ref().map(|p| p.valid_until),
        oidc_reauthentication_available: Some(oidc_available),
        oidc_organisation_id,
        organisation_id: policy.as_ref().map(|p| p.organisation_id),
        organisation_policy_revision: policy.as_ref().map(|p| p.revision),
        verification_expires_at: organisation_until
            .or_else(|| provider.as_ref().map(|p| p.valid_until)),
    })
}

#[derive(Default, serde::Deserialize)]
pub struct AssuranceQuery {
    pub organisation: Option<Uuid>,
    #[serde(default)]
    pub step_up: bool,
}

pub async fn status(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<AssuranceQuery>,
) -> Result<Json<AuthenticationAssurance>, ApiError> {
    let session = principal
        .session_id
        .ok_or_else(|| ApiError::Unauthorized("a user session is required".into()))?;
    Ok(Json(
        read(
            &state,
            principal.user_id,
            session,
            query.organisation,
            query.step_up,
        )
        .await?,
    ))
}

pub async fn authorize_request(
    state: &AppState,
    user: Uuid,
    session: Uuid,
    method: &Method,
    path: &str,
) -> Result<(), ApiError> {
    if crate::organisation_mfa::recovery_route(method, path) {
        return Ok(());
    }
    if !state.management_mode {
        return Ok(());
    }
    let status = read(
        state,
        user,
        session,
        crate::organisation_mfa::organisation_from_path(path),
        false,
    )
    .await?;
    if !status.required || status.session_verified {
        return Ok(());
    }
    let allowed = matches!(
        (method.as_str(), path),
        ("GET", "/api/v1/auth/assurance" | "/api/v1/auth/totp")
            | (
                "POST",
                "/api/v1/auth/assurance/verify" | "/api/v1/auth/logout"
            )
    ) || (status.enrollment_allowed
        && matches!(
            (method.as_str(), path),
            (
                "POST",
                "/api/v1/auth/totp/setup" | "/api/v1/auth/totp/enable"
            )
        ));
    if allowed {
        Ok(())
    } else {
        Err(ApiError::MfaRequired(
            "platform administration requires a verified second factor for this session".into(),
        ))
    }
}

pub async fn authorize_enrollment(state: &AppState, principal: &Principal) -> Result<(), ApiError> {
    crate::organisation_mfa::enrollment(state, principal).await?;
    if !state.management_mode {
        return Ok(());
    }
    let session = principal
        .session_id
        .ok_or_else(|| ApiError::Unauthorized("a user session is required".into()))?;
    if !read(state, principal.user_id, session, None, false)
        .await?
        .enrollment_allowed
    {
        return Err(ApiError::MfaRequired(
            "sign in again or verify an existing factor before enrolling an authenticator".into(),
        ));
    }
    Ok(())
}

async fn session_guard(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    session: Uuid,
) -> Result<(), ApiError> {
    crate::organisation_mfa::session_in_tx(state, tx, user, session, true).await?;
    Ok(())
}

pub async fn mark_verified(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    session: Uuid,
) -> Result<(), ApiError> {
    session_guard(state, tx, user, session).await?;
    let rows=sqlx::query("UPDATE auth_sessions SET second_factor_verified_at=now() WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL AND expires_at>now()")
        .bind(session).bind(user).execute(&mut **tx).await?.rows_affected();
    if rows != 1 {
        return Err(ApiError::Unauthorized("session is unavailable".into()));
    }
    Ok(())
}

pub async fn verify(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<AssuranceQuery>,
    Json(request): Json<VerifyTotpRequest>,
) -> Result<Json<AuthenticationAssurance>, ApiError> {
    if request.code.is_empty() || request.code.len() > 128 {
        return Err(ApiError::Unprocessable(
            "second-factor code must contain 1 to 128 characters".into(),
        ));
    }
    let session = principal
        .session_id
        .ok_or_else(|| ApiError::Unauthorized("a user session is required".into()))?;
    if !auth::verify_second_factor(&state, principal.user_id, Some(&request.code)).await? {
        return Err(ApiError::Conflict(
            "enrol an authenticator before verifying this session".into(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    mark_verified(&state, &mut tx, principal.user_id, session).await?;
    sqlx::query("INSERT INTO audit_events(id,actor_user_id,action,target_type,target_id) VALUES($1,$2,'auth.session.second_factor_verified','auth_session',$3)")
        .bind(Uuid::new_v4()).bind(principal.user_id).bind(session).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        read(
            &state,
            principal.user_id,
            session,
            query.organisation,
            query.step_up,
        )
        .await?,
    ))
}

/// Removal of different factor types must share a lock: concurrent deletions
/// must not each observe the other factor and leave a privileged account bare.
pub async fn protect_last_factor(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    removing_totp: bool,
) -> Result<(), ApiError> {
    crate::organisation_mfa::protect_last_factor(tx, user, removing_totp).await?;
    if !state.management_mode {
        return Ok(());
    }
    let admin: bool = sqlx::query_scalar("SELECT platform_admin FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut **tx)
        .await?;
    if !admin {
        return Ok(());
    }
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
        return Err(ApiError::Unprocessable(
            "platform administrators must retain an authenticator or a passkey".into(),
        ));
    }
    Ok(())
}
