//! Explicit remote Vault Transit custody. No file/environment discovery, cloud
//! provisioning, native-store fallback, default key, or automatic activation.
//! HTTP waits are async; callers fence their durable original operation before
//! IO and revalidate its epoch/current authority after each provider wait.
use crate::{Error, KeyId, KeyProof, KeyPurpose, KeyStore, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{
    Method, Url,
    header::{HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Duration};
use uuid::Uuid;
use zeroize::Zeroizing;

const MAX_REPLY: usize = 128 * 1024;
const MAX_ENVELOPES: usize = 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VaultTransitProfile {
    pub format: u32,
    pub provider_id: Uuid,
    /// Exact normalized HTTPS origin followed by '/', never an arbitrary URL.
    pub endpoint: String,
    pub mount: String,
    pub key_name: String,
    pub namespace: Option<String>,
}
fn component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
impl VaultTransitProfile {
    pub fn validate(&self) -> Result<()> {
        let url = Url::parse(&self.endpoint).map_err(|_| Error::Invalid)?;
        if self.format != 1
            || self.provider_id.is_nil()
            || self.endpoint.len() > 2048
            || url.scheme() != "https"
            || url.host_str().is_none_or(|h| h.contains('*'))
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.as_str() != self.endpoint
            || !self.mount.split('/').all(component)
            || self.mount.len() > 512
            || !component(&self.key_name)
            || self.namespace.as_ref().is_some_and(|n| {
                n.is_empty() || n.len() > 512 || !n.trim_end_matches('/').split('/').all(component)
            })
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    pub fn sha256(&self) -> Result<String> {
        self.validate()?;
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).map_err(|_| Error::Invalid)?)
        ))
    }
}

/// Ciphertext/confirmation metadata only. The original KeyId survives moving
/// custody to KMS and rewrapping under another version of the Vault wrapping key.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WrappedServerKey {
    pub format: u32,
    pub envelope_id: Uuid,
    pub provider_sha256: String,
    pub proof: KeyProof,
    pub context_base64: String,
    pub ciphertext: String,
    pub wrapping_version: u32,
}
fn context(profile_sha: &str, envelope_id: Uuid, proof: &KeyProof) -> Result<String> {
    proof.validate()?;
    if envelope_id.is_nil() {
        return Err(Error::Invalid);
    }
    Ok(STANDARD.encode(
        serde_json::to_vec(&(
            "cowork:server:kms-envelope:v1",
            profile_sha,
            envelope_id,
            proof,
        ))
        .map_err(|_| Error::Invalid)?,
    ))
}
fn ciphertext_version(value: &str) -> Result<u32> {
    if value.len() > 4096 {
        return Err(Error::Invalid);
    }
    let mut parts = value.split(':');
    if parts.next() != Some("vault") {
        return Err(Error::Invalid);
    }
    let version = parts
        .next()
        .and_then(|v| v.strip_prefix('v'))
        .ok_or(Error::Invalid)?;
    let version_number = version.parse::<u32>().map_err(|_| Error::Invalid)?;
    let cipher = parts.next().ok_or(Error::Invalid)?;
    if version_number == 0
        || version != version_number.to_string()
        || parts.next().is_some()
        || STANDARD.decode(cipher).map_err(|_| Error::Invalid)?.len() < 48
    {
        return Err(Error::Invalid);
    }
    Ok(version_number)
}
impl WrappedServerKey {
    pub fn validate(&self, profile: &VaultTransitProfile) -> Result<()> {
        let sha = profile.sha256()?;
        if self.format != 1
            || self.provider_sha256 != sha
            || self.context_base64 != context(&sha, self.envelope_id, &self.proof)?
            || ciphertext_version(&self.ciphertext)? != self.wrapping_version
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    pub fn sha256(&self) -> Result<String> {
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).map_err(|_| Error::Invalid)?)
        ))
    }
}

pub struct VaultTransit {
    profile: VaultTransitProfile,
    client: reqwest::Client,
}
#[derive(Deserialize)]
struct Response<T> {
    data: T,
}
#[derive(Deserialize)]
struct CipherReply {
    ciphertext: String,
}
#[derive(Deserialize)]
struct PlainReply {
    plaintext: Zeroizing<String>,
}
#[derive(Deserialize)]
struct MountPolicy {
    disable_upsert: bool,
}
#[derive(Deserialize)]
struct KeyPolicy {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    derived: bool,
    exportable: bool,
    allow_plaintext_backup: bool,
    deletion_allowed: bool,
    #[serde(default)]
    convergent_encryption: bool,
    min_decryption_version: u32,
    min_encryption_version: u32,
    keys: BTreeMap<String, serde_json::Value>,
}
impl KeyPolicy {
    fn permits(&self, name: &str, version: u32, encrypt: bool) -> Result<()> {
        if self.name != name
            || self.kind != "aes256-gcm96"
            || !self.derived
            || self.exportable
            || self.allow_plaintext_backup
            || self.deletion_allowed
            || self.convergent_encryption
            || self.keys.len() > 1024
            || version == 0
            || !self.keys.contains_key(&version.to_string())
            || version < self.min_decryption_version
            || encrypt && version < self.min_encryption_version
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
impl VaultTransit {
    /// Explicit private operator configuration supplies token/optional trusted
    /// CA. Construction performs no network IO and has no insecure HTTP mode.
    pub fn new(
        profile: VaultTransitProfile,
        token: Zeroizing<String>,
        trusted_ca_pem: Option<&[u8]>,
    ) -> Result<Self> {
        profile.validate()?;
        if !(16..=8192).contains(&token.len())
            || token
                .bytes()
                .any(|b| !b.is_ascii() || b.is_ascii_control() || b.is_ascii_whitespace())
        {
            return Err(Error::Invalid);
        }
        let mut headers = HeaderMap::new();
        let mut token_value = HeaderValue::from_str(&token).map_err(|_| Error::Invalid)?;
        token_value.set_sensitive(true);
        headers.insert("x-vault-token", token_value);
        if let Some(namespace) = &profile.namespace {
            headers.insert(
                "x-vault-namespace",
                HeaderValue::from_str(namespace).map_err(|_| Error::Invalid)?,
            );
        }
        let mut builder = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10));
        if let Some(pem) = trusted_ca_pem {
            if pem.len() > 1024 * 1024 {
                return Err(Error::Invalid);
            }
            builder = builder.add_root_certificate(
                reqwest::Certificate::from_pem(pem).map_err(|_| Error::Invalid)?,
            );
        }
        Ok(Self {
            profile,
            client: builder.build().map_err(|_| Error::Unavailable)?,
        })
    }
    pub fn profile(&self) -> &VaultTransitProfile {
        &self.profile
    }
    fn route(&self, operation: &str) -> Result<Url> {
        if !matches!(operation, "encrypt" | "decrypt" | "rewrap" | "keys") {
            return Err(Error::Invalid);
        }
        Url::parse(&format!(
            "{}v1/{}/{}/{}",
            self.profile.endpoint, self.profile.mount, operation, self.profile.key_name
        ))
        .map_err(|_| Error::Invalid)
    }
    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        body: Option<Zeroizing<Vec<u8>>>,
    ) -> Result<T> {
        let mut request = self.client.request(method, url);
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(body.to_vec());
        }
        // No retries: a caller reconciles its original frozen operation.
        let mut response = request.send().await.map_err(|_| Error::Unavailable)?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|n| n > MAX_REPLY as u64)
        {
            return Err(Error::Unavailable);
        }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
            if chunk.len() > MAX_REPLY - bytes.len() {
                return Err(Error::Invalid);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)
    }
    async fn policy(&self, version: u32, encrypt: bool) -> Result<()> {
        // Prevent Vault's encrypt upsert behavior. The application never
        // provisions a missing remote wrapping key or alters its policy.
        let url = Url::parse(&format!(
            "{}v1/{}/config/keys",
            self.profile.endpoint, self.profile.mount
        ))
        .map_err(|_| Error::Invalid)?;
        let mount: Response<MountPolicy> = self.request(Method::GET, url, None).await?;
        if !mount.data.disable_upsert {
            return Err(Error::Invalid);
        }
        let info: Response<KeyPolicy> =
            self.request(Method::GET, self.route("keys")?, None).await?;
        info.data.permits(&self.profile.key_name, version, encrypt)
    }
    /// The caller reserves envelope_id and original material durably BEFORE IO.
    /// This function returns an unactivated candidate, not a current key grant.
    pub async fn wrap(
        &self,
        envelope_id: Uuid,
        proof: KeyProof,
        material: Zeroizing<[u8; 32]>,
        version: u32,
    ) -> Result<WrappedServerKey> {
        crate::setup::verify_material(&proof, material.as_ref())?;
        let sha = self.profile.sha256()?;
        let derived = context(&sha, envelope_id, &proof)?;
        self.policy(version, true).await?;
        let plaintext = Zeroizing::new(STANDARD.encode(material.as_ref()));
        #[derive(Serialize)]
        struct Encrypt<'a> {
            plaintext: &'a str,
            context: &'a str,
            key_version: u32,
        }
        let request = Zeroizing::new(
            serde_json::to_vec(&Encrypt {
                plaintext: &plaintext,
                context: &derived,
                key_version: version,
            })
            .map_err(|_| Error::Invalid)?,
        );
        let response: Response<CipherReply> = self
            .request(Method::POST, self.route("encrypt")?, Some(request))
            .await?;
        let envelope = WrappedServerKey {
            format: 1,
            envelope_id,
            provider_sha256: sha,
            proof,
            context_base64: derived,
            ciphertext: response.data.ciphertext,
            wrapping_version: version,
        };
        envelope.validate(&self.profile)?;
        let verified = self.unwrap(&envelope).await?;
        if *verified != *material {
            return Err(Error::Invalid);
        }
        Ok(envelope)
    }
    pub async fn unwrap(&self, envelope: &WrappedServerKey) -> Result<Zeroizing<[u8; 32]>> {
        envelope.validate(&self.profile)?;
        self.policy(envelope.wrapping_version, false).await?;
        #[derive(Serialize)]
        struct Decrypt<'a> {
            ciphertext: &'a str,
            context: &'a str,
        }
        let request = Zeroizing::new(
            serde_json::to_vec(&Decrypt {
                ciphertext: &envelope.ciphertext,
                context: &envelope.context_base64,
            })
            .map_err(|_| Error::Invalid)?,
        );
        let response: Response<PlainReply> = self
            .request(Method::POST, self.route("decrypt")?, Some(request))
            .await?;
        let plain = Zeroizing::new(
            STANDARD
                .decode(response.data.plaintext.as_bytes())
                .map_err(|_| Error::Invalid)?,
        );
        if plain.len() != 32 {
            return Err(Error::Invalid);
        }
        let mut material = Zeroizing::new([0u8; 32]);
        material.copy_from_slice(&plain);
        crate::setup::verify_material(&envelope.proof, material.as_ref())?;
        Ok(material)
    }
    /// Preserve original seed/KeyId/proof/context. Remote rewrap never changes
    /// application signatures or existing data ciphertext/receipt identities.
    pub async fn rewrap(
        &self,
        envelope: &WrappedServerKey,
        version: u32,
    ) -> Result<WrappedServerKey> {
        envelope.validate(&self.profile)?;
        if version <= envelope.wrapping_version {
            return Err(Error::Invalid);
        }
        self.policy(version, true).await?;
        #[derive(Serialize)]
        struct Rewrap<'a> {
            ciphertext: &'a str,
            context: &'a str,
            key_version: u32,
        }
        let request = Zeroizing::new(
            serde_json::to_vec(&Rewrap {
                ciphertext: &envelope.ciphertext,
                context: &envelope.context_base64,
                key_version: version,
            })
            .map_err(|_| Error::Invalid)?,
        );
        let response: Response<CipherReply> = self
            .request(Method::POST, self.route("rewrap")?, Some(request))
            .await?;
        let mut next = envelope.clone();
        next.ciphertext = response.data.ciphertext;
        next.wrapping_version = version;
        next.validate(&self.profile)?;
        let _confirmed = self.unwrap(&next).await?;
        Ok(next)
    }
    /// Startup/recovery only, before requests/workers are admitted. The caller
    /// rechecks its immutable registry+active epoch AFTER these KMS waits.
    /// Loaded data keys live only in Zeroizing RAM; revocation takes effect when
    /// this exact snapshot is dropped/reloaded, never via a plaintext fallback.
    pub async fn confirm_keys(&self, envelopes: &[WrappedServerKey]) -> Result<ConfirmedKmsKeys> {
        if envelopes.is_empty() || envelopes.len() > MAX_ENVELOPES {
            return Err(Error::Invalid);
        }
        let mut keys = BTreeMap::new();
        let mut proofs = BTreeMap::new();
        for envelope in envelopes {
            let id = envelope.proof.key.cipher_id()?;
            if keys.contains_key(&id) {
                return Err(Error::Conflict);
            }
            let value = self.unwrap(envelope).await?;
            proofs.insert(id.clone(), envelope.proof.clone());
            keys.insert(id, value);
        }
        Ok(ConfirmedKmsKeys {
            keys,
            proofs,
            stable: None,
        })
    }
}
/// A proved immutable runtime snapshot; it cannot provision, import, change
/// active identity, fetch environment values, or silently acquire another key.
pub struct ConfirmedKmsKeys {
    keys: BTreeMap<String, Zeroizing<[u8; 32]>>,
    proofs: BTreeMap<String, KeyProof>,
    stable: Option<StableKeyPurposes>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StableKeyPurposes {
    pub audit: KeyId,
    pub offline: KeyId,
    pub request_fingerprint: KeyId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_audit: Option<KeyId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_epoch: Option<i64>,
}
impl ConfirmedKmsKeys {
    /// Only the original CAS-fenced registry supplies these immutable slots.
    /// A loaded active encryption key never implicitly becomes an audit signer.
    pub fn bind_stable_purposes(mut self, purposes: StableKeyPurposes) -> Result<Self> {
        for id in [
            purposes.audit,
            purposes.offline,
            purposes.request_fingerprint,
            purposes.operator_audit.unwrap_or(purposes.audit),
        ] {
            id.validate()?;
            if !self.keys.contains_key(&id.cipher_id()?) {
                return Err(Error::Missing);
            }
        }
        if purposes.audit.instance != purposes.offline.instance
            || purposes.audit.instance != purposes.request_fingerprint.instance
            || purposes.audit.instance != purposes.operator_audit.unwrap_or(purposes.audit).instance
            || purposes.audit_epoch.is_some_and(|epoch| epoch <= 0)
            || purposes.audit_epoch.is_some() != purposes.operator_audit.is_some()
        {
            return Err(Error::Invalid);
        }
        self.stable = Some(purposes);
        Ok(self)
    }
}
impl KeyStore for ConfirmedKmsKeys {
    fn load(&self, id: &KeyId) -> Result<Zeroizing<[u8; 32]>> {
        let locator = id.cipher_id()?;
        let key = self.keys.get(&locator).ok_or(Error::Missing)?;
        let proof = self.proofs.get(&locator).ok_or(Error::Invalid)?;
        crate::setup::verify_material(proof, key.as_ref())?;
        Ok(Zeroizing::new(**key))
    }
    fn key_for_purpose(&self, id: &KeyId, purpose: KeyPurpose) -> Result<KeyId> {
        if !self.keys.contains_key(&id.cipher_id()?) {
            return Err(Error::Missing);
        }
        let stable = self.stable.as_ref().ok_or(Error::Unavailable)?;
        Ok(match purpose {
            KeyPurpose::DataEncryption => *id,
            KeyPurpose::AuditSigning => stable.audit,
            KeyPurpose::OfflineSigning => stable.offline,
            KeyPurpose::RequestFingerprint => stable.request_fingerprint,
            KeyPurpose::OperatorSigning => stable.operator_audit.unwrap_or(stable.audit),
        })
    }
    fn audit_signing_epoch(&self, id: &KeyId) -> Result<i64> {
        if !self.keys.contains_key(&id.cipher_id()?) {
            return Err(Error::Missing);
        }
        Ok(self
            .stable
            .as_ref()
            .ok_or(Error::Unavailable)?
            .audit_epoch
            .unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Original(KeyId);
    impl KeyStore for Original {
        fn load(&self, id: &KeyId) -> Result<Zeroizing<[u8; 32]>> {
            if id != &self.0 {
                return Err(Error::Missing);
            }
            Ok(Zeroizing::new([79; 32]))
        }
    }
    #[test]
    fn exact_provider_context_and_original_key_identity_reject_substitution() {
        let profile = VaultTransitProfile {
            format: 1,
            provider_id: Uuid::from_u128(141),
            endpoint: "https://vault.example:8200/".into(),
            mount: "transit".into(),
            key_name: "cowork-v2".into(),
            namespace: None,
        };
        profile.validate().unwrap();
        let id = KeyId {
            instance: Uuid::from_u128(142),
            key: Uuid::from_u128(143),
        };
        let proof = crate::prove_key(&Original(id), &id).unwrap();
        let sha = profile.sha256().unwrap();
        let mut envelope = WrappedServerKey {
            format: 1,
            envelope_id: Uuid::from_u128(144),
            provider_sha256: sha.clone(),
            proof: proof.clone(),
            context_base64: context(&sha, Uuid::from_u128(144), &proof).unwrap(),
            ciphertext: format!("vault:v3:{}", STANDARD.encode([7; 60])),
            wrapping_version: 3,
        };
        envelope.validate(&profile).unwrap();
        envelope.proof.key.key = Uuid::from_u128(145);
        assert!(envelope.validate(&profile).is_err());
        let mut other = profile.clone();
        other.endpoint = "https://foreign.example/".into();
        assert!(envelope.validate(&other).is_err());
        for endpoint in [
            "http://vault.example/",
            "https://vault.example/path",
            "https://user:secret@vault.example/",
            "https://vault.example/?x=1",
        ] {
            other.endpoint = endpoint.into();
            assert!(other.validate().is_err());
        }
        assert!(ciphertext_version("vault:v03:aaaa").is_err());
        let body = serde_json::to_string(&envelope).unwrap();
        assert!(!body.contains("plaintext") && !body.contains("token"));
    }
}
