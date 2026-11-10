//! CORS for the native WebView. Cookie sessions remain same-origin only.
use anyhow::{bail, Context, Result};
use axum::http::{header, HeaderValue, Method};
use tower_http::cors::{AllowOrigin, CorsLayer};

pub fn layer(extra_origins: &str) -> Result<CorsLayer> {
    let mut origins = vec![
        HeaderValue::from_static("tauri://localhost"),
        HeaderValue::from_static("http://tauri.localhost"),
        HeaderValue::from_static("https://tauri.localhost"),
    ];
    for origin in extra_origins
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let url = reqwest::Url::parse(origin).context("invalid COWORK_DESKTOP_ORIGINS origin")?;
        let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || url.origin().ascii_serialization() != origin
        {
            bail!("COWORK_DESKTOP_ORIGINS requires exact HTTPS origins or HTTP loopback origins");
        }
        origins.push(HeaderValue::from_str(origin)?);
    }
    Ok(CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::HeaderName::from_static("last-event-id"),
        ])
        // Deliberately no allow_credentials: native clients use bearer tokens.
        .max_age(std::time::Duration::from_secs(600)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_explicit_origins() {
        assert!(layer("").is_ok());
        assert!(layer("http://127.0.0.1:5173,https://desktop.example.test").is_ok());
        for value in [
            "*",
            "null",
            "http://remote.example",
            "https://example.test/path",
            "https://example.test/",
            "https://user:secret@example.test",
            "https://example.test?x=1",
        ] {
            assert!(layer(value).is_err(), "accepted {value}");
        }
    }
}
