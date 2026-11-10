use super::*;
use std::sync::Arc;

/// Private native owner capability. Provisioning never overwrites an existing key.
pub trait NativeKeyManager: KeyStore {
    fn create(&self, id: &KeyId) -> Result<()>;
    /// Explicit recovery only; existing different material must never be overwritten.
    fn restore(&self, _proof: &KeyProof, _material: Zeroizing<[u8; 32]>) -> Result<()> {
        Err(Error::Unavailable)
    }
    fn retire(&self, _proof: &KeyProof) -> Result<()> {
        Err(Error::Unavailable)
    }
}
pub trait NativeKeyProvider: Send + Sync {
    fn open(&self, instance: Uuid) -> Result<Arc<dyn NativeKeyManager>>;
}
pub struct DesktopKeyProvider;
impl NativeKeyProvider for DesktopKeyProvider {
    fn open(&self, instance: Uuid) -> Result<Arc<dyn NativeKeyManager>> {
        Ok(Arc::new(NativeKeyStore::open(instance)?))
    }
}
impl NativeKeyManager for NativeKeyStore {
    fn create(&self, id: &KeyId) -> Result<()> {
        self.provision_at(id)
    }
    fn restore(&self, proof: &KeyProof, material: Zeroizing<[u8; 32]>) -> Result<()> {
        self.restore_at(proof, material)
    }
    fn retire(&self, proof: &KeyProof) -> Result<()> {
        match verify_key(self, proof) {
            Ok(()) => self.delete(&proof.key),
            Err(Error::Missing) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// Nonsecret key-confirmation tag, bound to the exact native instance and key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyProof {
    pub key: KeyId,
    tag: [u8; 32],
}
impl KeyProof {
    pub fn validate(&self) -> Result<()> {
        self.key.validate()
    }
}
fn message(id: &KeyId) -> Result<Vec<u8>> {
    id.validate()?;
    serde_json::to_vec(&("localai-cowork-v2:native-key-confirmation", 1, id))
        .map_err(|_| Error::Invalid)
}
pub(crate) fn verify_material(proof: &KeyProof, material: &[u8]) -> Result<()> {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, material);
    ring::hmac::verify(&key, &message(&proof.key)?, &proof.tag).map_err(|_| Error::Invalid)
}
pub fn prove_key(store: &dyn KeyStore, id: &KeyId) -> Result<KeyProof> {
    let input = message(id)?;
    let material = store.load(id)?;
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, material.as_ref());
    let mut tag = [0; 32];
    tag.copy_from_slice(ring::hmac::sign(&key, &input).as_ref());
    Ok(KeyProof { key: *id, tag })
}
pub fn verify_key(store: &dyn KeyStore, proof: &KeyProof) -> Result<()> {
    load_verified_key(store, proof).map(drop)
}
pub fn load_verified_key(store: &dyn KeyStore, proof: &KeyProof) -> Result<Zeroizing<[u8; 32]>> {
    let input = message(&proof.key)?;
    let material = store.load(&proof.key)?;
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, material.as_ref());
    ring::hmac::verify(&key, &input, &proof.tag).map_err(|_| Error::Invalid)?;
    Ok(material)
}
