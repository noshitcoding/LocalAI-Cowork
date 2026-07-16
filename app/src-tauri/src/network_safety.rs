use futures_util::StreamExt;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use reqwest::redirect::Policy;
use reqwest::StatusCode;
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};
use url::{Host, Url};

const MAX_URL_LENGTH: usize = 4_096;
const MAX_REDIRECTS: usize = 5;
const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const DNS_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT: &str = "OpenCowork/0.1 secure-web-fetch";

pub(crate) const MAX_TEXT_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct SafeTextResponse {
    pub final_url: String,
    pub status: StatusCode,
    pub content_type: String,
    pub body: String,
    pub truncated: bool,
}

struct ResolvedTarget {
    url: Url,
    dns_domain: Option<String>,
    addresses: Vec<SocketAddr>,
}

pub(crate) async fn fetch_public_text(
    input: &str,
    max_bytes: usize,
) -> Result<SafeTextResponse, String> {
    let max_bytes = max_bytes.clamp(1_024, MAX_TEXT_RESPONSE_BYTES);
    let started = Instant::now();
    let mut current_url = parse_public_web_url(input)?;
    let mut visited = HashSet::new();
    let mut redirect_count = 0usize;

    loop {
        if !visited.insert(current_url.as_str().to_string()) {
            return Err("network policy blocked a redirect loop".to_string());
        }

        let remaining = TOTAL_TIMEOUT
            .checked_sub(started.elapsed())
            .filter(|value| !value.is_zero())
            .ok_or_else(|| "web request timed out".to_string())?;
        let resolved = resolve_public_target(current_url, remaining.min(DNS_TIMEOUT)).await?;

        let mut client_builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .connect_timeout(CONNECT_TIMEOUT.min(remaining))
            .user_agent(USER_AGENT);
        if let Some(domain) = resolved.dns_domain.as_deref() {
            client_builder = client_builder.resolve_to_addrs(domain, &resolved.addresses);
        }
        let client = client_builder.build().map_err(|err| err.to_string())?;
        let response = client
            .get(resolved.url.clone())
            .timeout(remaining)
            .send()
            .await
            .map_err(|err| sanitized_request_error(&err))?;
        let status = response.status();

        if is_followable_redirect(status) {
            if redirect_count >= MAX_REDIRECTS {
                return Err(format!(
                    "network policy blocked more than {MAX_REDIRECTS} redirects"
                ));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .ok_or_else(|| "redirect response is missing Location".to_string())?
                .to_str()
                .map_err(|_| "redirect Location is not valid text".to_string())?;
            let next_url = resolve_redirect(&resolved.url, location)?;
            redirect_count += 1;
            current_url = next_url;
            continue;
        }

        if status.is_redirection() {
            return Err(format!("unsupported redirect status {}", status.as_u16()));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(normalize_content_type);
        if status != StatusCode::NO_CONTENT {
            let Some(content_type) = content_type.as_deref() else {
                return Err("network policy requires a textual Content-Type".to_string());
            };
            if !is_allowed_text_content_type(content_type) {
                return Err(format!(
                    "network policy blocked non-text Content-Type {content_type}"
                ));
            }
        }

        let advertised_length = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        let mut truncated = advertised_length
            .map(|length| length > max_bytes as u64)
            .unwrap_or(false);
        let mut bytes = Vec::with_capacity(
            advertised_length
                .map(|length| length.min(max_bytes as u64) as usize)
                .unwrap_or(0),
        );
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| sanitized_stream_error(&err))?;
            if append_with_limit(&mut bytes, &chunk, max_bytes) {
                truncated = true;
                break;
            }
            if bytes.len() == max_bytes {
                if stream
                    .next()
                    .await
                    .transpose()
                    .map_err(|err| sanitized_stream_error(&err))?
                    .is_some()
                {
                    truncated = true;
                }
                break;
            }
        }

        return Ok(SafeTextResponse {
            final_url: resolved.url.to_string(),
            status,
            content_type: content_type.unwrap_or_default(),
            body: String::from_utf8_lossy(&bytes).into_owned(),
            truncated,
        });
    }
}

pub(crate) fn origin_for_audit(input: &str) -> String {
    let Ok(url) = Url::parse(input) else {
        return "invalid-url".to_string();
    };
    let Some(host) = url.host_str() else {
        return "invalid-url".to_string();
    };
    match url.port() {
        Some(port) => format!("{}://{}:{}", url.scheme(), host, port),
        None => format!("{}://{}", url.scheme(), host),
    }
}

fn parse_public_web_url(input: &str) -> Result<Url, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("url must not be empty".to_string());
    }
    if trimmed.len() > MAX_URL_LENGTH {
        return Err(format!(
            "network policy blocks URLs longer than {MAX_URL_LENGTH} bytes"
        ));
    }

    let mut url = Url::parse(trimmed).map_err(|err| format!("invalid URL: {err}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("network policy allows only http and https URLs".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("network policy blocks credentials in URLs".to_string());
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "URL has no supported port".to_string())?;
    if !matches!(port, 80 | 443) {
        return Err(format!("network policy blocks destination port {port}"));
    }

    match url.host() {
        Some(Host::Domain(domain)) => validate_domain_name(domain)?,
        Some(Host::Ipv4(address)) => validate_public_ip(IpAddr::V4(address))?,
        Some(Host::Ipv6(address)) => validate_public_ip(IpAddr::V6(address))?,
        None => return Err("URL must include a host".to_string()),
    }

    url.set_fragment(None);
    Ok(url)
}

async fn resolve_public_target(url: Url, timeout: Duration) -> Result<ResolvedTarget, String> {
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "URL has no supported port".to_string())?;

    match url.host() {
        Some(Host::Domain(domain)) => {
            let domain = domain.to_string();
            validate_domain_name(&domain)?;
            let lookup =
                tokio::time::timeout(timeout, tokio::net::lookup_host((domain.as_str(), port)))
                    .await
                    .map_err(|_| "DNS lookup timed out".to_string())?
                    .map_err(|err| format!("DNS lookup failed: {err}"))?;

            let mut unique_ips = HashSet::new();
            let mut addresses = Vec::new();
            for address in lookup {
                let ip = address.ip();
                validate_public_ip(ip)?;
                if unique_ips.insert(ip) {
                    addresses.push(SocketAddr::new(ip, 0));
                }
            }
            if addresses.is_empty() {
                return Err("DNS lookup returned no addresses".to_string());
            }

            Ok(ResolvedTarget {
                url,
                dns_domain: Some(domain),
                addresses,
            })
        }
        Some(Host::Ipv4(address)) => {
            validate_public_ip(IpAddr::V4(address))?;
            Ok(ResolvedTarget {
                url,
                dns_domain: None,
                addresses: Vec::new(),
            })
        }
        Some(Host::Ipv6(address)) => {
            validate_public_ip(IpAddr::V6(address))?;
            Ok(ResolvedTarget {
                url,
                dns_domain: None,
                addresses: Vec::new(),
            })
        }
        None => Err("URL must include a host".to_string()),
    }
}

fn resolve_redirect(current: &Url, location: &str) -> Result<Url, String> {
    let next = current
        .join(location)
        .map_err(|err| format!("invalid redirect URL: {err}"))?;
    let next = parse_public_web_url(next.as_str())?;
    if current.scheme() == "https" && next.scheme() == "http" {
        return Err("network policy blocks HTTPS-to-HTTP redirects".to_string());
    }
    Ok(next)
}

fn validate_domain_name(domain: &str) -> Result<(), String> {
    let normalized = domain.trim_end_matches('.').to_ascii_lowercase();
    if normalized.is_empty()
        || matches!(normalized.as_str(), "localhost" | "localhost.localdomain")
        || [
            ".localhost",
            ".local",
            ".localdomain",
            ".internal",
            ".home",
            ".lan",
        ]
        .iter()
        .any(|suffix| normalized.ends_with(suffix))
    {
        return Err(format!("network policy blocks local hostname {normalized}"));
    }
    Ok(())
}

fn validate_public_ip(address: IpAddr) -> Result<(), String> {
    let is_public = match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    };
    if is_public {
        Ok(())
    } else {
        Err(format!(
            "network policy blocks non-public address {address}"
        ))
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    let segments = address.segments();
    if segments[..5].iter().all(|segment| *segment == 0) && segments[5] == 0xffff {
        let mapped = Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            segments[6] as u8,
            (segments[7] >> 8) as u8,
            segments[7] as u8,
        );
        return is_public_ipv4(mapped);
    }

    let in_global_unicast = segments[0] & 0xe000 == 0x2000;
    let ietf_special = segments[0] == 0x2001 && segments[1] <= 0x01ff;
    let documentation = (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || (segments[0] == 0x3fff && segments[1] <= 0x0fff);
    let transition_6to4 = segments[0] == 0x2002;

    in_global_unicast && !ietf_special && !documentation && !transition_6to4
}

fn is_followable_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn append_with_limit(buffer: &mut Vec<u8>, chunk: &[u8], max_bytes: usize) -> bool {
    let remaining_capacity = max_bytes.saturating_sub(buffer.len());
    let accepted = chunk.len().min(remaining_capacity);
    buffer.extend_from_slice(&chunk[..accepted]);
    accepted < chunk.len()
}

fn sanitized_request_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "web request timed out".to_string()
    } else if error.is_connect() {
        "web request could not connect to the validated origin".to_string()
    } else {
        "web request failed".to_string()
    }
}

fn sanitized_stream_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "web response timed out".to_string()
    } else {
        "web response stream failed".to_string()
    }
}

fn normalize_content_type(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn is_allowed_text_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || matches!(
            content_type,
            "application/json"
                | "application/xml"
                | "application/xhtml+xml"
                | "application/rss+xml"
                | "application/atom+xml"
        )
        || content_type.ends_with("+json")
        || content_type.ends_with("+xml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_policy_allows_public_http_and_https_origins() {
        let parsed = parse_public_web_url("https://example.com/path?q=1#fragment").unwrap();
        assert_eq!(parsed.as_str(), "https://example.com/path?q=1");
        assert!(parse_public_web_url("http://8.8.8.8/").is_ok());
        assert!(parse_public_web_url("https://[2606:4700:4700::1111]/").is_ok());
    }

    #[test]
    fn url_policy_rejects_credentials_local_targets_and_unsafe_ports() {
        for candidate in [
            "file:///etc/passwd",
            "ftp://example.com/file",
            "https://user:secret@example.com/",
            "http://localhost/",
            "http://service.internal/",
            "http://example.com:8080/",
            "http://127.0.0.1/",
            "http://2130706433/",
            "http://10.0.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://100.64.0.1/",
            "http://192.168.1.1/",
            "http://198.18.0.1/",
            "http://203.0.113.1/",
            "http://224.0.0.1/",
            "http://[::1]/",
            "http://[fc00::1]/",
            "http://[fe80::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[2001:db8::1]/",
            "http://[2002:7f00:1::]/",
        ] {
            assert!(
                parse_public_web_url(candidate).is_err(),
                "{candidate} should be blocked"
            );
        }
    }

    #[test]
    fn redirect_policy_revalidates_targets_and_blocks_downgrades() {
        let current = parse_public_web_url("https://example.com/start").unwrap();
        assert_eq!(
            resolve_redirect(&current, "/next").unwrap().as_str(),
            "https://example.com/next"
        );
        assert!(resolve_redirect(&current, "http://example.com/next").is_err());
        assert!(resolve_redirect(&current, "https://127.0.0.1/secret").is_err());
    }

    #[test]
    fn content_type_policy_accepts_text_and_rejects_binary_payloads() {
        assert!(is_allowed_text_content_type("text/html"));
        assert!(is_allowed_text_content_type("application/problem+json"));
        assert!(is_allowed_text_content_type("application/rss+xml"));
        assert!(!is_allowed_text_content_type("application/octet-stream"));
        assert!(!is_allowed_text_content_type("image/png"));
    }

    #[test]
    fn byte_limit_never_buffers_more_than_the_configured_cap() {
        let mut buffer = Vec::new();
        assert!(!append_with_limit(&mut buffer, b"1234", 6));
        assert!(append_with_limit(&mut buffer, b"5678", 6));
        assert_eq!(buffer, b"123456");
        assert!(append_with_limit(&mut buffer, b"9", 6));
        assert_eq!(buffer.len(), 6);
    }

    #[test]
    fn audit_origin_never_contains_paths_queries_or_fragments() {
        assert_eq!(
            origin_for_audit("https://example.com/private/token?api_key=secret#fragment"),
            "https://example.com"
        );
    }
}
