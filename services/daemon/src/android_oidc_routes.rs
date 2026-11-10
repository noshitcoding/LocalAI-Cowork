//! Browser-cookie and V1 native clients share original SDK flows and separate original transport receipts.
use super::*;
use axum::routing::put;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/auth/oidc/providers", get(providers))
        .route("/auth/oidc/profiles", get(profiles).post(create))
        .route("/auth/oidc/profiles/{profile}", put(update))
        .route("/auth/oidc/flows", post(begin))
        .route("/auth/oidc/complete", post(complete))
        .route("/auth/oidc/recover", post(recover))
        .route("/auth/oidc/retire", post(retire))
        .route("/auth/oidc/links", get(links))
        .route("/auth/oidc/links/{external}/revoke", post(unlink))
        .route("/auth/oidc/origin", get(origin))
        .route(
            "/auth/organisations/{organization}/oidc/settings",
            get(sso_settings).put(sso_write),
        )
        .route(
            "/auth/organisations/{organization}/oidc/membership-origins",
            get(sso_memberships),
        )
        .route("/auth/oidc/config", get(legacy_config))
        .route("/auth/oidc/start", post(legacy_login))
        .route("/auth/oidc/link/start", post(legacy_link))
        .route(
            "/auth/organisations/{organization}/oidc/config",
            get(legacy_org_config),
        )
        .route(
            "/auth/organisations/{organization}/oidc/start",
            post(legacy_org_login),
        )
        .route(
            "/auth/organisations/{organization}/oidc/link/start",
            post(legacy_org_link),
        )
        .route("/auth/oidc/callback", get(legacy_callback))
        .layer(DefaultBodyLimit::max(65536))
}
async fn sso_settings(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(org): Path<Uuid>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .identity()
            .android_oidc_sso_settings(&a, org)
            .await
            .map_err(identity_error)?
    )))
}
async fn sso_memberships(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(org): Path<Uuid>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .identity()
            .android_oidc_membership_origins(&a, org)
            .await
            .map_err(identity_error)?
    )))
}
async fn sso_write(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(org): Path<Uuid>,
    headers: HeaderMap,
    Json(q): Json<AndroidOidcSsoWrite>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .identity()
            .android_oidc_sso_write(
                &a,
                org,
                auth_security::operation(&headers)?,
                &q,
                s.keys.as_ref(),
                s.key_id
            )
            .await
            .map_err(identity_error)?
    )))
}
fn legacy_origin(s: &CompatState) -> Result<(String, i64)> {
    let config = s.devices.current();
    let origin = config.origin.clone().ok_or(Error::Forbidden)?;
    let parsed = reqwest::Url::parse(&origin).map_err(|_| Error::Forbidden)?;
    if parsed.scheme() != "https" || parsed.origin().ascii_serialization() != origin {
        return Err(Error::Forbidden);
    }
    Ok((origin, config.revision))
}
async fn legacy_public(s: CompatState, organization: Option<Uuid>) -> Answer {
    let callback = legacy_origin(&s)
        .ok()
        .map(|(origin, _)| format!("{origin}/api/v1/auth/oidc/callback"));
    Ok(Json(
        s.service
            .store()
            .identity()
            .android_oidc_compat_config(organization, callback.as_deref())
            .await
            .map_err(identity_error)?,
    ))
}
async fn legacy_config(State(s): State<CompatState>) -> Answer {
    legacy_public(s, None).await
}
async fn legacy_org_config(State(s): State<CompatState>, Path(org): Path<Uuid>) -> Answer {
    legacy_public(s, Some(org)).await
}
async fn legacy_start(
    s: CompatState,
    organization: Option<Uuid>,
    actor: Option<Principal>,
    q: AndroidOidcCompatStart,
) -> Answer {
    let (origin, revision) = legacy_origin(&s)?;
    if q.redirect_uri != "open-cowork://auth/callback"
        && q.redirect_uri != format!("{origin}/auth/callback")
    {
        return Err(Error::Forbidden.into());
    }
    let value = s
        .service
        .store()
        .identity()
        .android_oidc_compat_start(
            actor.as_ref(),
            organization,
            &q,
            &format!("{origin}/api/v1/auth/oidc/callback"),
            revision,
            s.keys.clone(),
            s.key_id,
        )
        .await
        .map_err(identity_error)?;
    if legacy_origin(&s)? != (origin, revision) {
        return Err(Error::Conflict.into());
    }
    Ok(Json(
        json!({"schema_version": 2, "authorization_url": value.authorization_url, "expires_at": iso(value.expires_at)?}),
    ))
}
async fn legacy_login(
    State(s): State<CompatState>,
    Json(q): Json<AndroidOidcCompatStart>,
) -> Answer {
    legacy_start(s, None, None, q).await
}
async fn legacy_link(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(q): Json<AndroidOidcCompatStart>,
) -> Answer {
    legacy_start(s, None, Some(a), q).await
}
async fn legacy_org_login(
    State(s): State<CompatState>,
    Path(org): Path<Uuid>,
    Json(q): Json<AndroidOidcCompatStart>,
) -> Answer {
    legacy_start(s, Some(org), None, q).await
}
async fn legacy_org_link(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(org): Path<Uuid>,
    Json(q): Json<AndroidOidcCompatStart>,
) -> Answer {
    legacy_start(s, Some(org), Some(a), q).await
}
async fn legacy_callback(
    State(s): State<CompatState>,
    headers: HeaderMap,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> std::result::Result<Response, Failure> {
    let (origin, revision) = legacy_origin(&s)?;
    let expected = reqwest::Url::parse(&origin).map_err(|_| Error::Forbidden)?;
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .ok_or(Error::Forbidden)?;
    let expected_host = match expected.port() {
        Some(port) => format!("{}:{port}", expected.host_str().ok_or(Error::Forbidden)?),
        None => expected.host_str().ok_or(Error::Forbidden)?.into(),
    };
    if headers.get_all(header::HOST).iter().count() != 1 || host != expected_host {
        return Err(Error::Forbidden.into());
    }
    let raw = raw.ok_or_else(invalid)?;
    if raw.len() > 16384 {
        return Err(invalid().into());
    }
    let query =
        reqwest::Url::parse(&format!("https://callback.invalid/?{raw}")).map_err(|_| invalid())?;
    let values = query
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect::<Vec<_>>();
    if values
        .iter()
        .any(|(k, _)| !matches!(k.as_str(), "code" | "state" | "iss" | "session_state"))
        || values
            .iter()
            .any(|(key, _)| values.iter().filter(|(k, _)| k == key).count() != 1)
    {
        return Err(invalid().into());
    }
    let get = |key: &str| {
        values
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    let (finish, redirect, client_state) = s
        .service
        .store()
        .identity()
        .android_oidc_compat_callback(
            get("state").ok_or_else(invalid)?,
            get("code").ok_or_else(invalid)?,
            get("iss"),
            revision,
            s.keys.clone(),
        )
        .await
        .map_err(identity_error)?;
    if redirect != "open-cowork://auth/callback" && redirect != format!("{origin}/auth/callback") {
        return Err(Error::Forbidden.into());
    }
    let original = s
        .service
        .store()
        .android_transport_oidc_finish(s.keys.clone(), s.key_id, &finish, None)
        .await?;
    if legacy_origin(&s)? != (origin, revision) {
        return Err(Error::Conflict.into());
    }
    // Re-read current profile/link/flow facts after the exchange and key waits.
    s.service
        .store()
        .identity()
        .android_oidc_compat_callback(
            get("state").ok_or_else(invalid)?,
            get("code").ok_or_else(invalid)?,
            get("iss"),
            revision,
            s.keys.clone(),
        )
        .await
        .map_err(identity_error)?;
    let code = original["code"].as_str().ok_or_else(invalid)?;
    let mut target = reqwest::Url::parse(&redirect).map_err(|_| invalid())?;
    target
        .query_pairs_mut()
        .append_pair("code", code)
        .append_pair("state", &client_state);
    let mut response = axum::response::Redirect::to(target.as_str()).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        "referrer-policy",
        axum::http::HeaderValue::from_static("no-referrer"),
    );
    Ok(response)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    organization_id: Option<Uuid>,
}
async fn providers(State(s): State<CompatState>, Query(q): Query<Scope>) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .identity()
            .android_oidc_profiles(None, q.organization_id, false)
            .await
            .map_err(identity_error)?
    )))
}
async fn profiles(
    State(s): State<CompatState>,
    Session(a): Session,
    Query(q): Query<Scope>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .identity()
            .android_oidc_profiles(Some(&a), q.organization_id, true)
            .await
            .map_err(identity_error)?
    )))
}
async fn create(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(q): Json<AndroidOidcProfileWrite>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .identity()
            .android_oidc_profile_write(&a, None, &q, s.keys.as_ref(), s.key_id)
            .await
            .map_err(identity_error)?
    )))
}
async fn update(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(q): Json<AndroidOidcProfileWrite>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .identity()
            .android_oidc_profile_write(&a, Some(id), &q, s.keys.as_ref(), s.key_id)
            .await
            .map_err(identity_error)?
    )))
}
async fn begin(
    State(s): State<CompatState>,
    headers: HeaderMap,
    Json(q): Json<AndroidOidcBegin>,
) -> Answer {
    let actor = if q.purpose == "link" {
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(Error::Forbidden)?;
        if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
            return Err(Error::Forbidden.into());
        }
        Some(
            s.service
                .store()
                .identity()
                .authenticate_session(token)
                .await
                .map_err(identity_error)?,
        )
    } else {
        None
    };
    Ok(Json(json!(
        s.service
            .store()
            .identity()
            .android_oidc_begin(actor.as_ref(), &q, s.keys.clone(), s.key_id)
            .await
            .map_err(identity_error)?
    )))
}
async fn complete(
    State(s): State<CompatState>,
    headers: HeaderMap,
    Json(q): Json<AndroidOidcFinish>,
) -> Answer {
    let revision = if browser_auth::mode(&headers)? {
        Some(browser_auth::origin(&s, &headers)?)
    } else {
        None
    };
    if revision.is_some() && q.native_code_challenge.is_some() {
        return Err(invalid().into());
    }
    Ok(Json(
        s.service
            .store()
            .android_transport_oidc_finish(s.keys.clone(), s.key_id, &q, revision)
            .await?,
    ))
}
async fn recover(State(s): State<CompatState>, Json(q): Json<AndroidOidcRecover>) -> Answer {
    Ok(Json(
        s.service
            .store()
            .identity()
            .android_oidc_recover(&q, s.keys.clone())
            .await
            .map_err(identity_error)?,
    ))
}
async fn retire(State(s): State<CompatState>, Json(q): Json<AndroidOidcRetire>) -> Answer {
    Ok(Json(
        s.service
            .store()
            .identity()
            .android_oidc_retire(&q, s.keys.clone(), s.key_id)
            .await
            .map_err(identity_error)?,
    ))
}
async fn links(State(s): State<CompatState>, Session(a): Session) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .identity()
            .android_oidc_links(&a)
            .await
            .map_err(identity_error)?
    )))
}
async fn unlink(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(q): Json<AndroidOidcUnlink>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .identity()
            .android_oidc_unlink(&a, id, &q, s.keys.as_ref(), s.key_id)
            .await
            .map_err(identity_error)?,
    ))
}
async fn origin(State(s): State<CompatState>, Session(a): Session) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .identity()
            .android_oidc_origin(&a)
            .await
            .map_err(identity_error)?
    )))
}
