//! Original V1 same-origin browser boundary. Refresh credentials never enter JavaScript.
use super::*;
use axum::body::{Body, to_bytes};
use axum::http::HeaderValue;
const COOKIE: &str = "__Host-cowork_refresh";
pub(super) fn mode(headers: &HeaderMap) -> Result<bool> {
    let mut modes = headers.get_all("x-cowork-session-mode").iter();
    let first = modes.next();
    if modes.next().is_some() {
        return Err(invalid());
    }
    match first {
        None => Ok(false),
        Some(v) if v == "browser-cookie" => Ok(true),
        _ => Err(invalid()),
    }
}
pub(super) fn origin(s: &CompatState, headers: &HeaderMap) -> Result<i64> {
    let config = s.devices.current();
    let expected = config.origin.clone().ok_or(Error::Forbidden)?;
    let url = reqwest::Url::parse(&expected).map_err(|_| Error::Forbidden)?;
    if url.scheme() != "https" {
        return Err(Error::Forbidden);
    }
    let mut origins = headers.get_all(header::ORIGIN).iter();
    let supplied = origins
        .next()
        .and_then(|v| v.to_str().ok())
        .ok_or(Error::Forbidden)?;
    if origins.next().is_some() || supplied != url.origin().ascii_serialization() {
        return Err(Error::Forbidden);
    }
    if headers
        .get("sec-fetch-site")
        .is_some_and(|v| v != "same-origin")
    {
        return Err(Error::Forbidden);
    }
    // Authority and trusted public origin are configured by the operator. No
    // forwarded headers or client-selected redirect establish cookie authority.
    let mut hosts = headers.get_all(header::HOST).iter();
    let host = hosts
        .next()
        .and_then(|v| v.to_str().ok())
        .ok_or(Error::Forbidden)?;
    let expected_host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().ok_or(Error::Forbidden)?),
        None => url.host_str().ok_or(Error::Forbidden)?.to_owned(),
    };
    if hosts.next().is_some() || host != expected_host {
        return Err(Error::Forbidden);
    }
    Ok(config.revision)
}
fn cookie(headers: &HeaderMap) -> Result<Zeroizing<String>> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE).iter() {
        let value = value.to_str().map_err(|_| Error::Forbidden)?;
        if value.len() > 8192 {
            return Err(Error::Forbidden);
        }
        for part in value.split(';').map(str::trim) {
            if let Some(token) = part.strip_prefix("__Host-cowork_refresh=") {
                if found.is_some() {
                    return Err(Error::Forbidden);
                }
                found = Some(Zeroizing::new(token.to_owned()));
            }
        }
    }
    found.ok_or(Error::Forbidden)
}
pub(super) async fn refresh(State(s): State<CompatState>, headers: HeaderMap) -> Answer {
    if !mode(&headers)? {
        return Err(Error::Forbidden.into());
    }
    let revision = origin(&s, &headers)?;
    let token = cookie(&headers)?;
    Ok(Json(
        s.service
            .store()
            .android_transport_exchange(
                s.keys.as_ref(),
                s.key_id,
                auth_security::operation(&headers)?,
                "refresh",
                &token,
                None,
                None,
                true,
                Some(revision),
                TTL,
            )
            .await?,
    ))
}
pub(super) async fn boundary(
    s: &CompatState,
    headers: &HeaderMap,
    path: &str,
    mut response: Response,
) -> Response {
    let enabled = mode(headers).unwrap_or(false);
    if !enabled || !path.contains("/auth/") {
        return response;
    }
    response.headers_mut().append(
        header::VARY,
        HeaderValue::from_static("x-cowork-session-mode"),
    );
    if path.ends_with("/auth/logout") && response.status().is_success() {
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_static(
                "__Host-cowork_refresh=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
            ),
        );
        return response;
    }
    if !response.status().is_success() {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, 65536).await {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let Some(token) = value
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(|v| Zeroizing::new(v.to_owned()))
    else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    if origin(s, headers).is_err() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let expiry = value["refresh_expires_at"]
        .as_str()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .map(|v| v.timestamp_millis())
        .unwrap_or(0);
    let max_age = expiry.saturating_sub(now()).div_euclid(1000).max(0);
    let cookie = format!(
        "{COOKIE}={}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={max_age}",
        token.as_str()
    );
    let Ok(cookie) = HeaderValue::from_str(&cookie) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    value
        .as_object_mut()
        .expect("token object")
        .remove("refresh_token");
    let Ok(body) = serde_json::to_vec(&value) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.append(header::SET_COOKIE, cookie);
    Response::from_parts(parts, Body::from(body))
}
