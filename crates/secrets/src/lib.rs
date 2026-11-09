//! Native key custody and encrypted, purpose-bound application secrets.
//! No default keystore, environment lookup, plaintext fallback or secret logging.
mod native;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;
pub use native::NativeKeyStore;
pub mod kms;
pub mod mcp_oauth;
pub use mcp_oauth::{McpSecretScope, SealedMcpBlob, open_mcp_blob, rewrap_mcp_blob, seal_mcp_blob};
mod recovery;
mod setup;
mod v1_audit;
pub use recovery::{KeyBackup, backup_keys, restore_keys};
use ring::{
    aead, hkdf,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
pub use setup::{
    DesktopKeyProvider, KeyProof, NativeKeyManager, NativeKeyProvider, load_verified_key,
    prove_key, verify_key,
};
use uuid::Uuid;
pub use v1_audit::NativeV1AuditKey;
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("secret_store_unavailable")]
    Unavailable,
    #[error("secret_key_missing")]
    Missing,
    #[error("secret_state_invalid")]
    Invalid,
    #[error("secret_store_conflict")]
    Conflict,
    #[error("secret_write_unconfirmed")]
    Unconfirmed,
}
pub type Result<T> = std::result::Result<T, Error>;

/// Nonsecret locator. Never a credential value or an authorization grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyId {
    pub instance: Uuid,
    pub key: Uuid,
}
impl KeyId {
    pub fn validate(&self) -> Result<()> {
        if self.instance.is_nil() || self.key.is_nil() {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    pub fn cipher_id(&self) -> Result<String> {
        self.validate()?;
        Ok(format!(
            "native.{}.{}",
            self.instance.simple(),
            self.key.simple()
        ))
    }
    pub fn from_cipher_id(value: &str) -> Result<Self> {
        let parts: Vec<_> = value.split('.').collect();
        if parts.len() != 3 || parts[0] != "native" {
            return Err(Error::Invalid);
        }
        let result = Self {
            instance: Uuid::parse_str(parts[1]).map_err(|_| Error::Invalid)?,
            key: Uuid::parse_str(parts[2]).map_err(|_| Error::Invalid)?,
        };
        if result.cipher_id()? != value {
            return Err(Error::Invalid);
        }
        Ok(result)
    }
}

/// Trusted adapter boundary. Native implementations persist only random keys.
/// Server KMS adapters must implement equivalent scope and availability checks.
pub trait KeyStore: Send + Sync {
    fn load(&self, id: &KeyId) -> Result<Zeroizing<[u8; 32]>>;
    /// Separately pinned original signing/fingerprint identities must survive
    /// data-encryption rotation. Existing single-key stores retain their exact
    /// caller-owned identity; KMS registries resolve only their proved slots.
    fn key_for_purpose(&self, id: &KeyId, _purpose: KeyPurpose) -> Result<KeyId> {
        id.validate()?;
        Ok(*id)
    }
    fn audit_signing_epoch(&self, id: &KeyId) -> Result<i64> {
        id.validate()?;
        Ok(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPurpose {
    DataEncryption,
    AuditSigning,
    OfflineSigning,
    RequestFingerprint,
    OperatorSigning,
}

/// Public metadata; every field is authenticated before returning plaintext.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialScope {
    pub project_id: String,
    pub server_id: String,
    pub reference: String,
    pub revision: i64,
    pub endpoint: String,
}
impl CredentialScope {
    pub fn validate(&self) -> Result<()> {
        if [&self.project_id, &self.server_id, &self.reference]
            .iter()
            .any(|v| v.is_empty() || v.len() > 128 || v.chars().any(char::is_control))
            || self.revision <= 0
            || self.endpoint.is_empty()
            || self.endpoint.len() > 2048
            || self.endpoint.chars().any(char::is_control)
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedCredential {
    version: u32,
    key_id: KeyId,
    salt: [u8; 32],
    ciphertext: Vec<u8>,
}
impl SealedCredential {
    pub fn key_id(&self) -> KeyId {
        self.key_id
    }
    pub fn validate(&self) -> Result<()> {
        self.key_id.validate()?;
        if self.version != 1 || !(17..=4112).contains(&self.ciphertext.len()) {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}

fn aad(scope: &CredentialScope, id: &KeyId) -> Result<Vec<u8>> {
    scope.validate()?;
    id.validate()?;
    serde_json::to_vec(&("localai-cowork-v2:mcp-bearer", 1, id, scope)).map_err(|_| Error::Invalid)
}
fn derived(master: &[u8], salt: &[u8; 32], aad: &[u8]) -> Result<aead::LessSafeKey> {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, salt);
    let prk = salt.extract(master);
    let info = [b"credential-key-v1".as_slice(), aad];
    let okm = prk
        .expand(&info, &aead::CHACHA20_POLY1305)
        .map_err(|_| Error::Invalid)?;
    let mut key = Zeroizing::new([0; 32]);
    okm.fill(key.as_mut()).map_err(|_| Error::Invalid)?;
    aead::UnboundKey::new(&aead::CHACHA20_POLY1305, key.as_ref())
        .map(aead::LessSafeKey::new)
        .map_err(|_| Error::Invalid)
}
pub fn validate_bearer(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 4096
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
/// Caller publishes the returned ciphertext and scope together in its transaction.
pub fn seal_bearer(
    store: &dyn KeyStore,
    id: KeyId,
    scope: &CredentialScope,
    bearer: &str,
) -> Result<SealedCredential> {
    validate_bearer(bearer)?;
    let aad = aad(scope, &id)?;
    let master = store.load(&id)?;
    let mut salt = [0; 32];
    SystemRandom::new()
        .fill(&mut salt)
        .map_err(|_| Error::Unavailable)?;
    let mut bytes = Zeroizing::new(bearer.as_bytes().to_vec());
    derived(master.as_ref(), &salt, &aad)?
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key([0; 12]),
            aead::Aad::from(aad),
            &mut *bytes,
        )
        .map_err(|_| Error::Invalid)?;
    Ok(SealedCredential {
        version: 1,
        key_id: id,
        salt,
        ciphertext: bytes.to_vec(),
    })
}
pub fn open_bearer(
    store: &dyn KeyStore,
    scope: &CredentialScope,
    sealed: &SealedCredential,
) -> Result<Zeroizing<String>> {
    sealed.validate()?;
    let aad = aad(scope, &sealed.key_id)?;
    let master = store.load(&sealed.key_id)?;
    let mut bytes = Zeroizing::new(sealed.ciphertext.clone());
    let plain = derived(master.as_ref(), &sealed.salt, &aad)?
        .open_in_place(
            aead::Nonce::assume_unique_for_key([0; 12]),
            aead::Aad::from(aad),
            &mut bytes,
        )
        .map_err(|_| Error::Invalid)?;
    let text = std::str::from_utf8(plain).map_err(|_| Error::Invalid)?;
    validate_bearer(text)?;
    Ok(Zeroizing::new(text.to_owned()))
}
