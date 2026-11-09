use super::*;
use std::sync::Mutex;

// Serialize this process's native keystore RPCs. No global default builder is used.
static NATIVE_LOCK: Mutex<()> = Mutex::new(());
pub struct NativeKeyStore {
    instance: Uuid,
}
impl NativeKeyStore {
    /// Opening never creates a key. Instance identity must come from native setup.
    pub fn open(instance: Uuid) -> Result<Self> {
        if instance.is_nil() {
            return Err(Error::Invalid);
        }
        Ok(Self { instance })
    }
    fn entry(&self, id: &KeyId) -> Result<platform::Entry> {
        id.validate()?;
        if id.instance != self.instance {
            return Err(Error::Invalid);
        }
        let service = format!("LocalAI-Cowork-V2.{}", self.instance.simple());
        let user = format!("key.{}", id.key.simple());
        platform::Entry::new(&service, &user)
    }
    /// Explicit provisioning only. Generates a new immutable random locator.
    /// Failed/uncertain publication is never retried under an existing locator.
    pub fn provision(&self) -> Result<KeyId> {
        let id = KeyId {
            instance: self.instance,
            key: Uuid::new_v4(),
        };
        self.provision_at(&id)?;
        Ok(id)
    }
    /// Called only with a durably reserved, fresh setup candidate by its owner.
    /// An existing entry is a conflict, including after an uncertain earlier write.
    pub(crate) fn provision_at(&self, id: &KeyId) -> Result<()> {
        let _lock = NATIVE_LOCK.lock().map_err(|_| Error::Unavailable)?;
        let entry = self.entry(id)?;
        match entry.get_secret() {
            Err(Error::Missing) => (),
            Ok(secret) => {
                drop(Zeroizing::new(secret));
                return Err(Error::Conflict);
            }
            Err(error) => return Err(error),
        }
        let mut key = Zeroizing::new([0; 32]);
        SystemRandom::new()
            .fill(key.as_mut())
            .map_err(|_| Error::Unavailable)?;
        let bytes = encode(id, key.as_ref());
        entry.set_secret(&bytes).map_err(|_| Error::Unconfirmed)?;
        let confirmed = Zeroizing::new(entry.get_secret().map_err(|_| Error::Unconfirmed)?);
        if *confirmed != *bytes {
            return Err(Error::Unconfirmed);
        }
        Ok(())
    }
    /// Explicit destructive owner action. No enumeration or bulk deletion.
    /// Caller must first prove no remaining ciphertext relies on this key.
    pub fn delete(&self, id: &KeyId) -> Result<()> {
        let _lock = NATIVE_LOCK.lock().map_err(|_| Error::Unavailable)?;
        let entry = self.entry(id)?;
        // A malformed or foreign entry is never silently removed.
        match entry.get_secret() {
            Ok(bytes) => {
                decode(id, Zeroizing::new(bytes))?;
            }
            Err(Error::Missing) => return Ok(()),
            Err(error) => return Err(error),
        }
        entry.delete_credential().map_err(|_| Error::Unconfirmed)?;
        match entry.get_secret() {
            Err(Error::Missing) => Ok(()),
            Ok(bytes) => {
                drop(Zeroizing::new(bytes));
                Err(Error::Unconfirmed)
            }
            Err(_) => Err(Error::Unconfirmed),
        }
    }
    pub(crate) fn restore_at(&self, proof: &KeyProof, material: Zeroizing<[u8; 32]>) -> Result<()> {
        super::setup::verify_material(proof, material.as_ref())?;
        let _lock = NATIVE_LOCK.lock().map_err(|_| Error::Unavailable)?;
        let entry = self.entry(&proof.key)?;
        let bytes = encode(&proof.key, material.as_ref());
        match entry.get_secret() {
            Ok(existing) => {
                let existing = Zeroizing::new(existing);
                return if *existing == *bytes {
                    Ok(())
                } else {
                    Err(Error::Conflict)
                };
            }
            Err(Error::Missing) => (),
            Err(error) => return Err(error),
        }
        entry.set_secret(&bytes).map_err(|_| Error::Unconfirmed)?;
        let confirmed = Zeroizing::new(entry.get_secret().map_err(|_| Error::Unconfirmed)?);
        if *confirmed != *bytes {
            return Err(Error::Unconfirmed);
        }
        Ok(())
    }
}
impl KeyStore for NativeKeyStore {
    fn load(&self, id: &KeyId) -> Result<Zeroizing<[u8; 32]>> {
        let _lock = NATIVE_LOCK.lock().map_err(|_| Error::Unavailable)?;
        decode(id, Zeroizing::new(self.entry(id)?.get_secret()?))
    }
}
fn encode(id: &KeyId, key: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(72));
    bytes.extend_from_slice(b"LACV2K01");
    bytes.extend_from_slice(id.instance.as_bytes());
    bytes.extend_from_slice(id.key.as_bytes());
    bytes.extend_from_slice(key);
    bytes
}
fn decode(id: &KeyId, bytes: Zeroizing<Vec<u8>>) -> Result<Zeroizing<[u8; 32]>> {
    if bytes.len() != 72
        || &bytes[..8] != b"LACV2K01"
        || &bytes[8..24] != id.instance.as_bytes()
        || &bytes[24..40] != id.key.as_bytes()
    {
        return Err(Error::Invalid);
    }
    let mut key = Zeroizing::new([0; 32]);
    key.copy_from_slice(&bytes[40..]);
    Ok(key)
}
