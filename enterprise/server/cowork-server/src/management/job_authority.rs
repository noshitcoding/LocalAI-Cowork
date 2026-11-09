//! Original session/assurance provenance for asynchronous operator plans.
use super::*;
use chrono::{DateTime, Utc};

fn origin(row: &sqlx::postgres::PgRow, user: Uuid, session: Uuid) -> Result<Value, ApiError> {
    Ok(json!({"version":1,"user_id":user,"session_id":session,
        "auth_origin":row.try_get::<Option<Value>,_>("auth_origin")?,
        "second_factor_verified_at":row.try_get::<Option<DateTime<Utc>>,_>("second_factor_verified_at")?,
        "oidc_assurance":row.try_get::<Option<Value>,_>("oidc_assurance")?}))
}
pub(super) async fn capture(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    principal: &Principal,
    org: Uuid,
) -> Result<Value, ApiError> {
    if principal.bootstrap {
        return Err(denied());
    }
    let session = principal.session_id.ok_or_else(denied)?;
    let row = crate::organisation_mfa::session_in_tx(state, tx, principal.user_id, session, false)
        .await?;
    let binding = origin(&row, principal.user_id, session)?;
    validate(state, tx, &binding, org).await?;
    Ok(binding)
}
pub(super) async fn validate(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    binding: &Value,
    org: Uuid,
) -> Result<(), ApiError> {
    if binding["version"] != 1 {
        return Err(denied());
    }
    let user = serde_json::from_value::<Uuid>(binding["user_id"].clone())?;
    let session = serde_json::from_value::<Uuid>(binding["session_id"].clone())?;
    let row = crate::organisation_mfa::session_in_tx(state, tx, user, session, false).await?;
    if origin(&row, user, session)? != *binding {
        return Err(ApiError::Conflict(
            "original management login or assurance changed".into(),
        ));
    }
    let policy = mfa::policy(tx, org).await?;
    if policy.required
        && !crate::organisation_mfa::assured_in_tx(state, tx, user, &row, &policy)
            .await?
            .0
    {
        return Err(ApiError::Unauthorized(
            "original management session does not meet current organisation MFA".into(),
        ));
    }
    Ok(())
}
