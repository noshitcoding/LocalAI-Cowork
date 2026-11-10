//! Configured fleet callbacks with optional private CA trust.
//! The opt-in is public CA material; it never carries TLS or operator keys.

use crate::error::ApiError;
use std::{io::Read, time::Duration};

const MAX_CA_BYTES: usize = 1024 * 1024;

fn unavailable() -> ApiError {
    ApiError::Unavailable("management transport configuration is unavailable".into())
}

fn builder(
    endpoint: &str,
    ca: Option<&[u8]>,
    timeout: Duration,
) -> Result<reqwest::ClientBuilder, ApiError> {
    let endpoint = reqwest::Url::parse(endpoint).map_err(|_| unavailable())?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || (ca.is_some() && endpoint.scheme() != "https")
    {
        return Err(unavailable());
    }
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(timeout.min(Duration::from_secs(10)))
        .timeout(timeout);
    if let Some(ca) = ca {
        if ca.is_empty()
            || ca.len() > MAX_CA_BYTES
            || ca
                .windows(b"PRIVATE KEY".len())
                .any(|part| part == b"PRIVATE KEY")
        {
            return Err(unavailable());
        }
        let certificates = reqwest::Certificate::from_pem_bundle(ca).map_err(|_| unavailable())?;
        if certificates.is_empty() {
            return Err(unavailable());
        }
        builder = builder.tls_built_in_root_certs(false);
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    Ok(builder)
}

pub(crate) fn client(endpoint: &str, timeout: Duration) -> Result<reqwest::Client, ApiError> {
    client_with_ca_file(endpoint, timeout, "COWORK_FLEET_CA_CERTIFICATE_FILE")
}

pub(crate) fn client_with_ca_file(
    endpoint: &str,
    timeout: Duration,
    ca_environment: &'static str,
) -> Result<reqwest::Client, ApiError> {
    let ca = match std::env::var(ca_environment) {
        Ok(path) => {
            let file = std::fs::File::open(path).map_err(|_| unavailable())?;
            let metadata = file.metadata().map_err(|_| unavailable())?;
            if !metadata.is_file() || metadata.len() > MAX_CA_BYTES as u64 {
                return Err(unavailable());
            }
            let mut ca = Vec::new();
            file.take(MAX_CA_BYTES as u64 + 1)
                .read_to_end(&mut ca)
                .map_err(|_| unavailable())?;
            Some(ca)
        }
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => return Err(unavailable()),
    };
    builder(endpoint, ca.as_deref(), timeout)?
        .build()
        .map_err(|_| unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_trust_cannot_downgrade_transport_or_accept_ambiguous_endpoint() {
        for endpoint in [
            "http://localhost",
            "https://user:password@localhost",
            "https://localhost?token=x",
            "https://localhost#fragment",
            "file:///tmp/service",
            "not a url",
        ] {
            assert!(builder(endpoint, Some(b"public CA"), Duration::from_secs(5)).is_err());
        }
        assert!(builder("http://127.0.0.1:8080", None, Duration::from_secs(5)).is_ok());
        assert!(builder("https://localhost", None, Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn malformed_empty_oversized_or_private_key_material_fails_closed() {
        for ca in [
            b"".as_slice(),
            b"not a certificate",
            b"-----BEGIN PRIVATE KEY-----",
        ] {
            assert!(builder("https://localhost", Some(ca), Duration::from_secs(5)).is_err());
        }
        assert!(builder(
            "https://localhost",
            Some(&vec![b'x'; MAX_CA_BYTES + 1]),
            Duration::from_secs(5)
        )
        .is_err());
    }
}
