//! Original V1 authentication security shapes over actual V2 subjects/sessions.
use super::*;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/auth/assurance", get(assurance))
        .route("/auth/security-context", get(context))
        .route("/auth/assurance/verify", post(verify))
        .route("/auth/sessions", get(sessions))
        .route("/auth/sessions/{session}", axum::routing::delete(revoke))
        .route("/auth/invitations", post(invitation))
        .route("/auth/invitations/accept", post(accept))
        .route("/auth/totp", get(totp))
        .route("/auth/totp/setup", post(setup))
        .route("/auth/totp/enable", post(enable))
        .route("/auth/totp/disable", post(disable))
        .route("/auth/totp/recovery-codes", post(recovery))
}
pub(super) fn operation(headers: &HeaderMap) -> Result<Uuid> {
    let mut values = headers.get_all("idempotency-key").iter();
    let first = values.next();
    if values.next().is_some() {
        return Err(invalid());
    }
    match first {
        Some(v) => {
            let text = v.to_str().map_err(|_| invalid())?;
            let id = Uuid::parse_str(text).map_err(|_| invalid())?;
            if id.is_nil() || id.to_string() != text {
                return Err(invalid());
            }
            Ok(id)
        }
        None => Ok(Uuid::new_v4()),
    }
}
type Code = AndroidTotpCode;
type Disable = AndroidDisableTotp;
type Invite = AndroidCreateInvitation;
type Accept = AndroidAcceptInvitation;
async fn assurance(State(s): State<CompatState>, Session(a): Session) -> Answer {
    Ok(Json(s.service.store().android_security_status(&a).await?))
}
async fn context(State(s): State<CompatState>, Session(a): Session) -> Answer {
    Ok(Json(s.service.store().android_security_context(&a).await?))
}
async fn totp(State(s): State<CompatState>, Session(a): Session) -> Answer {
    Ok(Json(
        s.service.store().android_security_totp_status(&a).await?,
    ))
}
async fn sessions(State(s): State<CompatState>, Session(a): Session) -> Answer {
    Ok(Json(json!(
        s.service.store().android_security_sessions(&a).await?
    )))
}
async fn revoke(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(session): Path<Uuid>,
) -> std::result::Result<StatusCode, Failure> {
    s.service
        .store()
        .android_security_revoke_session(&a, session)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn setup(State(s): State<CompatState>, Session(a): Session, headers: HeaderMap) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_security_totp_setup(s.keys.as_ref(), s.key_id, &a, operation(&headers)?)
            .await?,
    ))
}
async fn factor(
    s: CompatState,
    a: Principal,
    headers: HeaderMap,
    code: String,
    purpose: &str,
) -> Answer {
    let code = Zeroizing::new(code);
    Ok(Json(
        s.service
            .store()
            .android_security_factor_operation(
                s.keys.as_ref(),
                s.key_id,
                &a,
                operation(&headers)?,
                purpose,
                &code,
                None,
            )
            .await?,
    ))
}
async fn enable(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
    Json(v): Json<Code>,
) -> Answer {
    factor(s, a, headers, v.code, "totp_enable").await
}
async fn verify(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
    Json(v): Json<Code>,
) -> Answer {
    factor(s, a, headers, v.code, "assurance_verify").await
}
async fn recovery(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
    Json(v): Json<Code>,
) -> Answer {
    factor(s, a, headers, v.code, "recovery_codes").await
}
async fn disable(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
    Json(v): Json<Disable>,
) -> std::result::Result<StatusCode, Failure> {
    let password = Zeroizing::new(v.password);
    let code = Zeroizing::new(v.second_factor);
    s.service
        .store()
        .android_security_factor_operation(
            s.keys.as_ref(),
            s.key_id,
            &a,
            operation(&headers)?,
            "totp_disable",
            &code,
            Some(&password),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn invitation(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
    Json(v): Json<Invite>,
) -> std::result::Result<(StatusCode, Json<Value>), Failure> {
    let expires = v
        .expires_at
        .as_deref()
        .map(|t| {
            chrono::DateTime::parse_from_rfc3339(t)
                .map(|v| v.timestamp_millis())
                .map_err(|_| invalid())
        })
        .transpose()?;
    Ok((
        StatusCode::CREATED,
        Json(
            s.service
                .store()
                .android_security_create_invitation(
                    s.keys.as_ref(),
                    s.key_id,
                    &a,
                    &v.email,
                    expires,
                    operation(&headers)?,
                )
                .await?,
        ),
    ))
}
async fn accept(
    State(s): State<CompatState>,
    headers: HeaderMap,
    Json(v): Json<Accept>,
) -> std::result::Result<(StatusCode, Json<Value>), Failure> {
    let token = Zeroizing::new(v.token);
    let password = Zeroizing::new(v.password);
    Ok((
        StatusCode::CREATED,
        Json(
            s.service
                .store()
                .android_security_accept_invitation(
                    s.keys.as_ref(),
                    s.key_id,
                    &token,
                    &v.display_name,
                    &password,
                    v.device_id,
                    TTL,
                    operation(&headers)?,
                )
                .await?,
        ),
    ))
}
