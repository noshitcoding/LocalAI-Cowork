//! Explicit browser origin for a TLS proxy; the daemon still binds loopback.
use axum::http::Uri;

pub fn origins(port: u16, public: Option<&str>) -> anyhow::Result<Vec<String>> {
    let mut origins = vec![
        "http://127.0.0.1:5175".into(),
        "http://localhost:5175".into(),
        format!("http://127.0.0.1:{port}"),
        "http://tauri.localhost".into(),
        "tauri://localhost".into(),
    ];
    if let Some(public) = public {
        anyhow::ensure!(
            valid_public_origin(public),
            "COWORK_PUBLIC_ORIGIN must be an exact canonical HTTPS origin"
        );
        origins.push(public.to_owned());
    }
    Ok(origins)
}

fn valid_public_origin(value: &str) -> bool {
    let Ok(uri) = value.parse::<Uri>() else {
        return false;
    };
    let Some(authority) = uri.authority() else {
        return false;
    };
    if uri.scheme_str() != Some("https")
        || value != format!("https://{authority}")
        || authority.as_str().contains(['@', '*'])
    {
        return false;
    }
    let host = authority.host();
    // Uri::Authority::port() returns None for an out-of-range port, so inspect
    // the original authority suffix rather than silently treating it as absent.
    let Some(suffix) = authority.as_str().strip_prefix(host) else {
        return false;
    };
    if !suffix.is_empty() && !suffix.strip_prefix(':').is_some_and(|port| {
        matches!(port.parse::<u16>(), Ok(number) if number > 0 && number != 443 && number.to_string() == port)
    }) { return false; }
    if host.starts_with('[') {
        return host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .is_some_and(|h| h.parse::<std::net::Ipv6Addr>().is_ok());
    }
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_https_origin_is_exact_and_loopback_defaults_remain() {
        for value in [
            "https://cowork.example",
            "https://cowork.example:8443",
            "https://127.0.0.1",
            "https://[::1]:8443",
        ] {
            let list = origins(43821, Some(value)).unwrap();
            assert_eq!(list.last().unwrap(), value);
            assert!(list.iter().any(|v| v == "http://127.0.0.1:43821"));
            assert!(!list.iter().any(|v| v == "*"));
        }
        assert_eq!(origins(43821, None).unwrap().len(), 5);
    }
    #[test]
    fn malformed_noncanonical_and_wildcard_origins_fail_closed() {
        for value in [
            "",
            "*",
            "null",
            "http://cowork.example",
            "https://*.example",
            "https://user@cowork.example",
            "https://cowork.example/",
            "https://cowork.example/api",
            "https://cowork.example?token=private",
            "https://cowork.example#fragment",
            "https://COWORK.example",
            "https://cowork.example.",
            "https://cowork.example:443",
            "https://cowork.example:0",
            "https://cowork.example:0443",
            "https://cowork.example:65536",
            "https://cowork.example:invalid",
            "https://cowork..example",
            "https://-cowork.example",
            "https://cowork.example\n",
            " https://cowork.example",
        ] {
            assert!(
                origins(43821, Some(value)).is_err(),
                "invalid origin {value:?}"
            );
        }
    }
}
