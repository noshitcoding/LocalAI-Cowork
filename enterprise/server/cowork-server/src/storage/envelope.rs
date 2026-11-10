//! Scoped envelope encryption and key decoding, independent of network storage.

use crate::error::ApiError;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Copy)]
pub(super) enum KeyScope {
    User(Uuid),
    Team(Uuid),
    DirectoryConnector(Uuid),
}

impl KeyScope {
    pub(super) fn kind(self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::Team(_) => "team",
            Self::DirectoryConnector(_) => "directory_connector",
        }
    }

    pub(super) fn id(self) -> Uuid {
        match self {
            Self::User(id) | Self::Team(id) | Self::DirectoryConnector(id) => id,
        }
    }

    pub(super) fn key_id(self) -> String {
        format!("master-v1:{}:{}", self.kind(), self.id())
    }
}

pub(super) struct EncryptedChunk {
    pub(super) ciphertext: Vec<u8>,
    pub(super) wrapped_data_key: Vec<u8>,
    pub(super) nonce: [u8; 12],
    pub(super) wrap_nonce: [u8; 12],
}

#[derive(Debug)]
pub(crate) struct SealedValue {
    pub ciphertext: Vec<u8>,
    pub encrypted_data_key: Vec<u8>,
    pub nonce: [u8; 12],
    pub wrap_nonce: [u8; 12],
}

/// User-scoped envelope encryption independent of any network/file storage backend.
#[derive(Clone)]
pub struct SecretStore {
    pub(super) master_key: [u8; 32],
}

impl SecretStore {
    pub(crate) fn seal_for_directory(
        &self,
        connector: Uuid,
        plaintext: &[u8],
    ) -> Result<SealedValue, ApiError> {
        let encrypted = self.encrypt(KeyScope::DirectoryConnector(connector), plaintext)?;
        Ok(SealedValue {
            ciphertext: encrypted.ciphertext,
            encrypted_data_key: encrypted.wrapped_data_key,
            nonce: encrypted.nonce,
            wrap_nonce: encrypted.wrap_nonce,
        })
    }
    pub(crate) fn open_for_directory(
        &self,
        connector: Uuid,
        sealed: &SealedValue,
    ) -> Result<Vec<u8>, ApiError> {
        self.decrypt(
            KeyScope::DirectoryConnector(connector),
            &sealed.ciphertext,
            &sealed.encrypted_data_key,
            &sealed.nonce,
            &sealed.wrap_nonce,
        )
    }
    pub fn from_key(value: &str) -> anyhow::Result<Self> {
        Ok(Self {
            master_key: decode_master_key(value)?,
        })
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

    pub(super) fn encrypt(
        &self,
        scope: KeyScope,
        plaintext: &[u8],
    ) -> Result<EncryptedChunk, ApiError> {
        let mut data_key = [0_u8; 32];
        let mut nonce = [0_u8; 12];
        let mut wrap_nonce = [0_u8; 12];
        getrandom::fill(&mut data_key).map_err(|error| {
            ApiError::Internal(anyhow::anyhow!("random key generation failed: {error}"))
        })?;
        getrandom::fill(&mut nonce).map_err(|error| {
            ApiError::Internal(anyhow::anyhow!("random nonce generation failed: {error}"))
        })?;
        getrandom::fill(&mut wrap_nonce).map_err(|error| {
            ApiError::Internal(anyhow::anyhow!(
                "random wrap nonce generation failed: {error}"
            ))
        })?;
        let cipher = Aes256Gcm::new_from_slice(&data_key)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid generated data key")))?;
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("chunk encryption failed")))?;
        let scope_key = self.scope_key(scope)?;
        let wrapper = Aes256Gcm::new_from_slice(&scope_key)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid scope key")))?;
        let wrapped_data_key = wrapper
            .encrypt(Nonce::from_slice(&wrap_nonce), data_key.as_slice())
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("data-key wrapping failed")))?;
        data_key.fill(0);
        Ok(EncryptedChunk {
            ciphertext,
            wrapped_data_key,
            nonce,
            wrap_nonce,
        })
    }

    pub(super) fn decrypt(
        &self,
        scope: KeyScope,
        ciphertext: &[u8],
        wrapped_data_key: &[u8],
        nonce: &[u8],
        wrap_nonce: &[u8],
    ) -> Result<Vec<u8>, ApiError> {
        if nonce.len() != 12 || wrap_nonce.len() != 12 {
            return Err(ApiError::Internal(anyhow::anyhow!(
                "invalid encrypted chunk nonce"
            )));
        }
        let scope_key = self.scope_key(scope)?;
        let wrapper = Aes256Gcm::new_from_slice(&scope_key)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid scope key")))?;
        let mut data_key = wrapper
            .decrypt(Nonce::from_slice(wrap_nonce), wrapped_data_key)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("data-key unwrap failed")))?;
        let cipher = Aes256Gcm::new_from_slice(&data_key)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid unwrapped data key")))?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("chunk authentication failed")))?;
        data_key.fill(0);
        Ok(plaintext)
    }

    fn scope_key(&self, scope: KeyScope) -> Result<[u8; 32], ApiError> {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.master_key)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid storage master key")))?;
        mac.update(b"open-cowork-envelope-v1\0");
        mac.update(scope.kind().as_bytes());
        mac.update(b"\0");
        mac.update(scope.id().as_bytes());
        Ok(mac.finalize().into_bytes().into())
    }
}

pub(super) fn decode_master_key(value: &str) -> anyhow::Result<[u8; 32]> {
    let trimmed = value.trim();
    let decoded = if trimmed.len() == 64 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        hex::decode(trimmed)?
    } else {
        STANDARD
            .decode(trimmed)
            .or_else(|_| URL_SAFE_NO_PAD.decode(trimmed))?
    };
    anyhow::ensure!(
        decoded.len() == 32,
        "storage master key must decode to exactly 32 bytes"
    );
    let mut key = [0_u8; 32];
    key.copy_from_slice(&decoded);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentication_secrets_are_independent_scoped_and_legacy_compatible() {
        let bytes = [17_u8; 32];
        let secret = SecretStore::from_key(&hex::encode(bytes)).unwrap();
        let same = SecretStore::from_key(&STANDARD.encode(bytes)).unwrap();
        let other = SecretStore::from_key(&STANDARD.encode([18_u8; 32])).unwrap();
        let user = Uuid::new_v4();
        let sealed = secret.seal_for_user(user, b"authenticator-secret").unwrap();
        assert_eq!(
            same.open_for_user(user, &sealed).unwrap(),
            b"authenticator-secret"
        );
        assert!(same.open_for_user(Uuid::new_v4(), &sealed).is_err());
        assert!(other.open_for_user(user, &sealed).is_err());
        assert!(SecretStore::from_key("short").is_err());
        let mut corrupt = sealed;
        corrupt.ciphertext[0] ^= 1;
        assert!(secret.open_for_user(user, &corrupt).is_err());
    }
}
