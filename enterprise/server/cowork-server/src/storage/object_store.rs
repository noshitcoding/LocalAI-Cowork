//! S3 configuration, request signing and encrypted object access.

use crate::{
    config::ObjectStoreConfig,
    error::ApiError,
    storage::envelope::{decode_master_key, EncryptedChunk, KeyScope, SealedValue, SecretStore},
    AppState,
};
use chrono::{Datelike, Timelike, Utc};
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::{Client, Method, Url};
use sha2::{Digest, Sha256};
use std::{net::IpAddr, sync::Arc, time::Duration as StdDuration};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

pub(super) const AWS_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

#[derive(Clone)]
pub struct ObjectStore {
    s3: S3Client,
    master_key: [u8; 32],
}

#[derive(Clone)]
struct S3Client {
    http: Client,
    endpoint: Url,
    region: String,
    bucket: String,
    addressing_style: S3AddressingStyle,
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum S3AddressingStyle {
    Path,
    VirtualHosted,
}

impl ObjectStore {
    pub fn from_config(config: &ObjectStoreConfig) -> anyhow::Result<Self> {
        let master_key = decode_master_key(&config.master_key)?;
        let endpoint = Url::parse(config.endpoint.trim())?;
        anyhow::ensure!(
            matches!(endpoint.scheme(), "http" | "https"),
            "S3 endpoint must use HTTP or HTTPS"
        );
        anyhow::ensure!(
            endpoint.query().is_none() && endpoint.fragment().is_none(),
            "S3 endpoint cannot contain query or fragment"
        );
        anyhow::ensure!(
            endpoint.username().is_empty() && endpoint.password().is_none(),
            "S3 endpoint cannot contain credentials"
        );
        anyhow::ensure!(
            matches!(endpoint.path(), "" | "/"),
            "S3 endpoint cannot contain a path prefix"
        );
        anyhow::ensure!(
            endpoint.host_str().is_some(),
            "S3 endpoint must contain a host"
        );
        anyhow::ensure!(
            !config.bucket.trim().is_empty()
                && !config.bucket.contains('/')
                && !config.bucket.contains('\\')
                && !config.bucket.chars().any(char::is_control),
            "S3 bucket name is invalid"
        );
        let addressing_style = match config.addressing_style.trim() {
            "path" => S3AddressingStyle::Path,
            "virtual_hosted" | "virtual-hosted" => S3AddressingStyle::VirtualHosted,
            other => anyhow::bail!(
                "COWORK_S3_ADDRESSING_STYLE must be path or virtual_hosted; got {other}"
            ),
        };
        if addressing_style == S3AddressingStyle::VirtualHosted {
            let host = endpoint.host_str().unwrap_or_default();
            anyhow::ensure!(
                host.parse::<IpAddr>().is_err(),
                "virtual-hosted S3 addressing requires a DNS endpoint"
            );
            anyhow::ensure!(
                valid_virtual_host_bucket(&config.bucket),
                "virtual-hosted S3 addressing requires a DNS-compatible bucket name"
            );
        }
        let http = if std::env::var_os("COWORK_S3_CA_CERTIFICATE_FILE").is_some() {
            crate::management_transport::client_with_ca_file(
                endpoint.as_str(),
                StdDuration::from_secs(5 * 60),
                "COWORK_S3_CA_CERTIFICATE_FILE",
            )?
        } else {
            Client::builder()
                .connect_timeout(StdDuration::from_secs(10))
                .timeout(StdDuration::from_secs(5 * 60))
                .build()?
        };
        Ok(Self {
            s3: S3Client {
                http,
                endpoint,
                region: config.region.clone(),
                bucket: config.bucket.clone(),
                addressing_style,
                access_key: config.access_key.clone(),
                secret_key: config.secret_key.clone(),
                session_token: config.session_token.clone(),
            },
            master_key,
        })
    }

    pub(super) async fn put_encrypted(
        &self,
        scope: KeyScope,
        digest_hex: &str,
        plaintext: &[u8],
    ) -> Result<(String, EncryptedChunk), ApiError> {
        let encrypted = self.encrypt(scope, plaintext)?;
        let object_key = format!(
            "chunks/{}/{}/{}/{}",
            scope.kind(),
            scope.id(),
            &digest_hex[..2],
            digest_hex
        );
        crate::instance_disk_quotas::write_gate("objects", encrypted.ciphertext.len() as u64)
            .await?;
        self.s3
            .request(Method::PUT, &object_key, Some(encrypted.ciphertext.clone()))
            .await?;
        Ok((object_key, encrypted))
    }

    pub(super) async fn get_decrypted(
        &self,
        scope: KeyScope,
        object_key: &str,
        wrapped_data_key: &[u8],
        nonce: &[u8],
        wrap_nonce: &[u8],
    ) -> Result<Vec<u8>, ApiError> {
        let ciphertext = self.s3.request(Method::GET, object_key, None).await?;
        self.decrypt(scope, &ciphertext, wrapped_data_key, nonce, wrap_nonce)
    }

    pub(super) async fn delete(&self, object_key: &str) -> Result<(), ApiError> {
        self.s3.request(Method::DELETE, object_key, None).await?;
        Ok(())
    }

    pub(super) async fn put_ciphertext(
        &self,
        object_key: &str,
        ciphertext: Vec<u8>,
    ) -> Result<(), ApiError> {
        crate::instance_disk_quotas::write_gate("objects", ciphertext.len() as u64).await?;
        self.s3
            .request(Method::PUT, object_key, Some(ciphertext))
            .await?;
        Ok(())
    }

    pub(crate) fn seal_for_user(
        &self,
        user_id: Uuid,
        plaintext: &[u8],
    ) -> Result<SealedValue, ApiError> {
        let encrypted = self.encrypt(KeyScope::User(user_id), plaintext)?;
        Ok(SealedValue {
            ciphertext: encrypted.ciphertext,
            encrypted_data_key: encrypted.wrapped_data_key,
            nonce: encrypted.nonce,
            wrap_nonce: encrypted.wrap_nonce,
        })
    }

    pub(crate) fn open_for_user(
        &self,
        user_id: Uuid,
        sealed: &SealedValue,
    ) -> Result<Vec<u8>, ApiError> {
        self.decrypt(
            KeyScope::User(user_id),
            &sealed.ciphertext,
            &sealed.encrypted_data_key,
            &sealed.nonce,
            &sealed.wrap_nonce,
        )
    }

    pub(crate) fn seal_for_team(
        &self,
        team_id: Uuid,
        plaintext: &[u8],
    ) -> Result<SealedValue, ApiError> {
        let encrypted = self.encrypt(KeyScope::Team(team_id), plaintext)?;
        Ok(SealedValue {
            ciphertext: encrypted.ciphertext,
            encrypted_data_key: encrypted.wrapped_data_key,
            nonce: encrypted.nonce,
            wrap_nonce: encrypted.wrap_nonce,
        })
    }

    pub(crate) fn open_for_team(
        &self,
        team_id: Uuid,
        sealed: &SealedValue,
    ) -> Result<Vec<u8>, ApiError> {
        self.decrypt(
            KeyScope::Team(team_id),
            &sealed.ciphertext,
            &sealed.encrypted_data_key,
            &sealed.nonce,
            &sealed.wrap_nonce,
        )
    }

    pub(crate) fn secret_store(&self) -> SecretStore {
        SecretStore {
            master_key: self.master_key,
        }
    }
    pub(super) fn encrypt(
        &self,
        scope: KeyScope,
        plaintext: &[u8],
    ) -> Result<EncryptedChunk, ApiError> {
        self.secret_store().encrypt(scope, plaintext)
    }
    fn decrypt(
        &self,
        scope: KeyScope,
        ciphertext: &[u8],
        wrapped_data_key: &[u8],
        nonce: &[u8],
        wrap_nonce: &[u8],
    ) -> Result<Vec<u8>, ApiError> {
        self.secret_store()
            .decrypt(scope, ciphertext, wrapped_data_key, nonce, wrap_nonce)
    }
}

impl S3Client {
    fn request_location(&self, object_key: &str) -> Result<(Url, String, String), ApiError> {
        let encoded_key = object_key
            .split('/')
            .map(encode_segment)
            .collect::<Vec<_>>()
            .join("/");
        let canonical_uri = match self.addressing_style {
            S3AddressingStyle::Path if encoded_key.is_empty() => {
                format!("/{}", encode_segment(&self.bucket))
            }
            S3AddressingStyle::Path => {
                format!("/{}/{}", encode_segment(&self.bucket), encoded_key)
            }
            S3AddressingStyle::VirtualHosted if encoded_key.is_empty() => "/".to_owned(),
            S3AddressingStyle::VirtualHosted => format!("/{encoded_key}"),
        };
        let mut url = self.endpoint.clone();
        if self.addressing_style == S3AddressingStyle::VirtualHosted {
            let host = format!(
                "{}.{}",
                self.bucket,
                self.endpoint.host_str().unwrap_or_default()
            );
            url.set_host(Some(&host)).map_err(|_| {
                ApiError::Internal(anyhow::anyhow!("invalid virtual-hosted S3 endpoint"))
            })?;
        }
        url.set_path(&canonical_uri);
        let host = match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
            None => url.host_str().unwrap_or_default().to_owned(),
        };
        Ok((url, canonical_uri, host))
    }

    async fn request(
        &self,
        method: Method,
        object_key: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, ApiError> {
        let (parsed, canonical_uri, host) = self.request_location(object_key)?;
        let payload = body.as_deref().unwrap_or_default();
        let payload_hash = hex::encode(Sha256::digest(payload));
        let now = Utc::now();
        let amz_date = format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            now.year(),
            now.month(),
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        );
        let short_date = format!("{:04}{:02}{:02}", now.year(), now.month(), now.day());
        let (canonical_headers, signed_headers) = match &self.session_token {
            Some(token) => (
                format!(
                    "host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\nx-amz-security-token:{}\n",
                    token.trim()
                ),
                "host;x-amz-content-sha256;x-amz-date;x-amz-security-token",
            ),
            None => (
                format!(
                    "host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n"
                ),
                "host;x-amz-content-sha256;x-amz-date",
            ),
        };
        let canonical_request = format!(
            "{}\n{}\n\n{}\n{}\n{}",
            method.as_str(),
            canonical_uri,
            canonical_headers,
            signed_headers,
            payload_hash
        );
        let credential_scope = format!("{short_date}/{}/s3/aws4_request", self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );
        let signing_key = aws_signing_key(&self.secret_key, &short_date, &self.region)?;
        let signature = hex::encode(hmac_bytes(&signing_key, string_to_sign.as_bytes())?);
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key, credential_scope, signed_headers, signature
        );
        let mut request = self
            .http
            .request(method, parsed)
            .header("host", host)
            .header("x-amz-content-sha256", payload_hash)
            .header("x-amz-date", amz_date)
            .header("authorization", authorization);
        if let Some(token) = &self.session_token {
            request = request.header("x-amz-security-token", token.trim());
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        let response = request.send().await.map_err(|error| {
            ApiError::Internal(anyhow::anyhow!("object-store request failed: {error}"))
        })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|error| {
            ApiError::Internal(anyhow::anyhow!("object-store response failed: {error}"))
        })?;
        if !status.is_success() {
            let detail = String::from_utf8_lossy(&bytes);
            return Err(ApiError::Internal(anyhow::anyhow!(
                "object store returned {status}: {}",
                detail.chars().take(1000).collect::<String>()
            )));
        }
        Ok(bytes.to_vec())
    }
}

pub(super) fn require_store(state: &AppState) -> Result<Arc<ObjectStore>, ApiError> {
    state.object_store.clone().ok_or_else(|| {
        ApiError::Conflict("object storage is not configured on this server".to_owned())
    })
}

fn encode_segment(value: &str) -> String {
    utf8_percent_encode(value, AWS_ENCODE_SET).to_string()
}

fn valid_virtual_host_bucket(value: &str) -> bool {
    if !(3..=63).contains(&value.len()) || value.parse::<IpAddr>().is_ok() || value.contains("..") {
        return false;
    }
    value.split('.').all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            })
    })
}

fn hmac_bytes(key: &[u8], value: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key)
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid HMAC key")))?;
    mac.update(value);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn aws_signing_key(secret: &str, date: &str, region: &str) -> Result<Vec<u8>, ApiError> {
    let date_key = hmac_bytes(format!("AWS4{secret}").as_bytes(), date.as_bytes())?;
    let region_key = hmac_bytes(&date_key, region.as_bytes())?;
    let service_key = hmac_bytes(&region_key, b"s3")?;
    hmac_bytes(&service_key, b"aws4_request")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::extract::Request;
    use axum::http::HeaderMap;
    use axum::http::StatusCode;
    use axum::routing::any;
    use axum::Router;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use std::env;
    use std::net::SocketAddr;
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    #[test]
    fn envelope_roundtrip_and_scope_separation() {
        let store = ObjectStore {
            s3: S3Client {
                http: Client::new(),
                endpoint: Url::parse("http://127.0.0.1:9000").unwrap(),
                region: "us-east-1".to_owned(),
                bucket: "test".to_owned(),
                addressing_style: S3AddressingStyle::Path,
                access_key: "test".to_owned(),
                secret_key: "test".to_owned(),
                session_token: None,
            },
            master_key: [7; 32],
        };
        let scope = KeyScope::User(Uuid::new_v4());
        let encrypted = store.encrypt(scope, b"classified").unwrap();
        let plaintext = store
            .decrypt(
                scope,
                &encrypted.ciphertext,
                &encrypted.wrapped_data_key,
                &encrypted.nonce,
                &encrypted.wrap_nonce,
            )
            .unwrap();
        assert_eq!(plaintext, b"classified");
        assert!(store
            .decrypt(
                KeyScope::User(Uuid::new_v4()),
                &encrypted.ciphertext,
                &encrypted.wrapped_data_key,
                &encrypted.nonce,
                &encrypted.wrap_nonce,
            )
            .is_err());
    }

    #[test]
    fn signing_key_is_deterministic_and_region_scoped() {
        let key = aws_signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20120215",
            "us-east-1",
        )
        .unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(
            key,
            aws_signing_key(
                "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
                "20120215",
                "us-east-1"
            )
            .unwrap()
        );
        assert_ne!(
            key,
            aws_signing_key(
                "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
                "20120215",
                "eu-central-1"
            )
            .unwrap()
        );
    }

    #[test]
    fn builds_path_and_virtual_hosted_locations() {
        let path_client = S3Client {
            http: Client::new(),
            endpoint: Url::parse("https://s3.eu-central-1.amazonaws.com").unwrap(),
            region: "eu-central-1".to_owned(),
            bucket: "cowork-blobs".to_owned(),
            addressing_style: S3AddressingStyle::Path,
            access_key: "access".to_owned(),
            secret_key: "secret".to_owned(),
            session_token: None,
        };
        let (url, canonical, host) = path_client
            .request_location("chunks/ab/object name")
            .unwrap();
        assert_eq!(canonical, "/cowork-blobs/chunks/ab/object%20name");
        assert_eq!(
            url.as_str(),
            "https://s3.eu-central-1.amazonaws.com/cowork-blobs/chunks/ab/object%20name"
        );
        assert_eq!(host, "s3.eu-central-1.amazonaws.com");

        let virtual_client = S3Client {
            addressing_style: S3AddressingStyle::VirtualHosted,
            ..path_client
        };
        let (url, canonical, host) = virtual_client.request_location("chunks/ab/object").unwrap();
        assert_eq!(canonical, "/chunks/ab/object");
        assert_eq!(
            url.as_str(),
            "https://cowork-blobs.s3.eu-central-1.amazonaws.com/chunks/ab/object"
        );
        assert_eq!(host, "cowork-blobs.s3.eu-central-1.amazonaws.com");
    }

    #[test]
    fn validates_virtual_hosted_storage_configuration() {
        let base = ObjectStoreConfig {
            endpoint: "https://s3.eu-central-1.amazonaws.com".to_owned(),
            region: "eu-central-1".to_owned(),
            bucket: "cowork-blobs".to_owned(),
            addressing_style: "virtual_hosted".to_owned(),
            access_key: "access".to_owned(),
            secret_key: "secret".to_owned(),
            session_token: Some("temporary-session-token".to_owned()),
            master_key: STANDARD.encode([7_u8; 32]),
        };
        assert!(ObjectStore::from_config(&base).is_ok());

        let invalid_ip = ObjectStoreConfig {
            endpoint: "http://127.0.0.1:9000".to_owned(),
            ..base.clone()
        };
        assert!(ObjectStore::from_config(&invalid_ip).is_err());
        let invalid_bucket = ObjectStoreConfig {
            bucket: "Not_DNS_Compatible".to_owned(),
            ..base
        };
        assert!(ObjectStore::from_config(&invalid_bucket).is_err());
    }

    #[tokio::test]
    async fn sends_virtual_hosted_request_with_signed_session_token() {
        #[derive(Clone)]
        struct CapturedRequest {
            path: String,
            headers: HeaderMap,
            body: Vec<u8>,
        }

        let captured = Arc::new(Mutex::new(None::<CapturedRequest>));
        let capture_target = captured.clone();
        let app = Router::new().fallback(any(move |request: Request| {
            let capture_target = capture_target.clone();
            async move {
                let path = request.uri().path().to_owned();
                let headers = request.headers().clone();
                let body = to_bytes(request.into_body(), 1024).await.unwrap().to_vec();
                *capture_target.lock().unwrap() = Some(CapturedRequest {
                    path,
                    headers,
                    body,
                });
                StatusCode::OK
            }
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let virtual_host = "cowork-blobs.s3.localhost";
        let client = S3Client {
            http: Client::builder()
                .resolve(
                    virtual_host,
                    SocketAddr::from(([127, 0, 0, 1], address.port())),
                )
                .build()
                .unwrap(),
            endpoint: Url::parse(&format!("http://s3.localhost:{}", address.port())).unwrap(),
            region: "eu-central-1".to_owned(),
            bucket: "cowork-blobs".to_owned(),
            addressing_style: S3AddressingStyle::VirtualHosted,
            access_key: "temporary-access".to_owned(),
            secret_key: "temporary-secret".to_owned(),
            session_token: Some("temporary-session-token".to_owned()),
        };

        client
            .request(Method::PUT, "chunks/ab/object", Some(b"payload".to_vec()))
            .await
            .unwrap();
        let captured = captured.lock().unwrap().clone().unwrap();
        assert_eq!(captured.path, "/chunks/ab/object");
        assert_eq!(captured.body, b"payload");
        assert_eq!(
            captured.headers.get("host").unwrap(),
            format!("{virtual_host}:{}", address.port()).as_str()
        );
        assert_eq!(
            captured.headers.get("x-amz-security-token").unwrap(),
            "temporary-session-token"
        );
        let authorization = captured
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(authorization
            .contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token"));
        server.abort();
    }

    #[tokio::test]
    #[ignore = "requires explicit COWORK_S3_TEST_* credentials and mutates only the configured test bucket"]
    async fn external_s3_provider_acceptance() {
        let required = |name: &str| {
            env::var(name).unwrap_or_else(|_| panic!("missing acceptance-test variable {name}"))
        };
        let config = ObjectStoreConfig {
            endpoint: required("COWORK_S3_TEST_ENDPOINT"),
            region: required("COWORK_S3_TEST_REGION"),
            bucket: required("COWORK_S3_TEST_BUCKET"),
            addressing_style: env::var("COWORK_S3_TEST_ADDRESSING_STYLE")
                .unwrap_or_else(|_| "path".to_owned()),
            access_key: required("COWORK_S3_TEST_ACCESS_KEY"),
            secret_key: required("COWORK_S3_TEST_SECRET_KEY"),
            session_token: env::var("COWORK_S3_TEST_SESSION_TOKEN")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            master_key: STANDARD.encode([41_u8; 32]),
        };
        let store = ObjectStore::from_config(&config).unwrap();
        let scope = KeyScope::User(Uuid::new_v4());
        let plaintext = format!("open-cowork-s3-acceptance-{}", Uuid::new_v4()).into_bytes();
        let digest = hex::encode(Sha256::digest(&plaintext));
        let (object_key, encrypted) = store
            .put_encrypted(scope, &digest, &plaintext)
            .await
            .unwrap();
        let roundtrip = store
            .get_decrypted(
                scope,
                &object_key,
                &encrypted.wrapped_data_key,
                &encrypted.nonce,
                &encrypted.wrap_nonce,
            )
            .await
            .unwrap();
        assert_eq!(roundtrip, plaintext);
        store.delete(&object_key).await.unwrap();
    }
}
