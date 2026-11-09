//! Password-encrypted native keys. Plaintext never leaves this custody module.
use super::*;
use ring::pbkdf2;
use std::num::NonZeroU32;

const ITERATIONS: u32 = 600_000;
const MAX_KEYS: usize = 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyBackup {
    pub format: u32,
    pub instance: Uuid,
    pub proofs: Vec<KeyProof>,
    pub salt: [u8; 32],
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}
impl KeyBackup {
    fn validate(&self) -> Result<()> {
        if self.format != 1
            || self.instance.is_nil()
            || self.proofs.is_empty()
            || self.proofs.len() > MAX_KEYS
            || self.ciphertext.len() != self.proofs.len() * 32 + 16
        {
            return Err(Error::Invalid);
        }
        let mut previous = None;
        for proof in &self.proofs {
            proof.validate()?;
            if proof.key.instance != self.instance || previous.is_some_and(|id| id >= proof.key.key)
            {
                return Err(Error::Invalid);
            }
            previous = Some(proof.key.key);
        }
        Ok(())
    }
    fn aad(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&(
            "localai-cowork-v2:native-key-backup",
            self.format,
            self.instance,
            &self.proofs,
            self.salt,
            self.nonce,
        ))
        .map_err(|_| Error::Invalid)
    }
    fn cipher(&self, password: &str) -> Result<aead::LessSafeKey> {
        if password.len() < 12 || password.len() > 4096 {
            return Err(Error::Invalid);
        }
        let mut material = Zeroizing::new([0u8; 32]);
        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            NonZeroU32::new(ITERATIONS).ok_or(Error::Invalid)?,
            &self.salt,
            password.as_bytes(),
            material.as_mut(),
        );
        aead::UnboundKey::new(&aead::AES_256_GCM, material.as_ref())
            .map(aead::LessSafeKey::new)
            .map_err(|_| Error::Invalid)
    }
}

pub fn backup_keys(
    store: &dyn KeyStore,
    instance: Uuid,
    proofs: Vec<KeyProof>,
    password: Zeroizing<String>,
) -> Result<KeyBackup> {
    if proofs.is_empty() || proofs.len() > MAX_KEYS {
        return Err(Error::Invalid);
    }
    let mut backup = KeyBackup {
        format: 1,
        instance,
        proofs,
        salt: [0; 32],
        nonce: [0; 12],
        ciphertext: Vec::new(),
    };
    // Validate metadata and bounds before touching the OS store.
    backup.ciphertext.resize(backup.proofs.len() * 32 + 16, 0);
    backup.validate()?;
    SystemRandom::new()
        .fill(&mut backup.salt)
        .map_err(|_| Error::Unavailable)?;
    SystemRandom::new()
        .fill(&mut backup.nonce)
        .map_err(|_| Error::Unavailable)?;
    let mut plaintext = Zeroizing::new(Vec::with_capacity(backup.proofs.len() * 32 + 16));
    for proof in &backup.proofs {
        plaintext.extend_from_slice(load_verified_key(store, proof)?.as_ref());
    }
    backup
        .cipher(&password)?
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(backup.nonce),
            aead::Aad::from(backup.aad()?),
            &mut *plaintext,
        )
        .map_err(|_| Error::Invalid)?;
    backup.ciphertext = plaintext.to_vec();
    Ok(backup)
}

struct MaterialKey {
    id: KeyId,
    material: Zeroizing<[u8; 32]>,
}
impl KeyStore for MaterialKey {
    fn load(&self, id: &KeyId) -> Result<Zeroizing<[u8; 32]>> {
        if id != &self.id {
            return Err(Error::Invalid);
        }
        Ok(Zeroizing::new(*self.material))
    }
}
pub fn restore_keys(
    manager: &dyn NativeKeyManager,
    backup: &KeyBackup,
    expected: &[KeyProof],
    password: Zeroizing<String>,
) -> Result<usize> {
    backup.validate()?;
    if backup.proofs.iter().any(|proof| !expected.contains(proof)) {
        return Err(Error::Invalid);
    }
    let mut ciphertext = Zeroizing::new(backup.ciphertext.clone());
    let plaintext = backup
        .cipher(&password)?
        .open_in_place(
            aead::Nonce::assume_unique_for_key(backup.nonce),
            aead::Aad::from(backup.aad()?),
            &mut ciphertext,
        )
        .map_err(|_| Error::Invalid)?;
    // Authenticate every key and check existing entries BEFORE the first write.
    let mut keys = Vec::with_capacity(backup.proofs.len());
    for (proof, bytes) in backup.proofs.iter().zip(plaintext.chunks_exact(32)) {
        let mut material = Zeroizing::new([0; 32]);
        material.copy_from_slice(bytes);
        let key = MaterialKey {
            id: proof.key,
            material,
        };
        verify_key(&key, proof)?;
        match manager.load(&proof.key) {
            Ok(existing) if *existing == *key.material => (),
            Err(Error::Missing) => (),
            Ok(_) => return Err(Error::Conflict),
            Err(error) => return Err(error),
        }
        keys.push(key);
    }
    for (proof, key) in backup.proofs.iter().zip(keys) {
        manager.restore(proof, key.material)?;
        verify_key(manager, proof)?;
    }
    Ok(backup.proofs.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    struct Memory(Mutex<Option<(KeyId, [u8; 32])>>);
    impl KeyStore for Memory {
        fn load(&self, id: &KeyId) -> Result<Zeroizing<[u8; 32]>> {
            self.0
                .lock()
                .unwrap()
                .as_ref()
                .filter(|(key, _)| key == id)
                .map(|(_, key)| Zeroizing::new(*key))
                .ok_or(Error::Missing)
        }
    }
    impl NativeKeyManager for Memory {
        fn create(&self, _: &KeyId) -> Result<()> {
            Err(Error::Invalid)
        }
        fn restore(&self, proof: &KeyProof, key: Zeroizing<[u8; 32]>) -> Result<()> {
            let mut existing = self.0.lock().unwrap();
            if existing.as_ref().is_some_and(|(_, bytes)| *bytes != *key) {
                return Err(Error::Conflict);
            }
            *existing = Some((proof.key, *key));
            Ok(())
        }
    }
    #[test]
    fn encrypted_recovery_preserves_original_identity_and_rejects_overwrite() {
        let id = KeyId {
            instance: Uuid::new_v4(),
            key: Uuid::new_v4(),
        };
        let source = Memory(Mutex::new(Some((id, [37; 32]))));
        let proof = prove_key(&source, &id).unwrap();
        let backup = backup_keys(
            &source,
            id.instance,
            vec![proof.clone()],
            Zeroizing::new("private recovery password".into()),
        )
        .unwrap();
        let destination = Memory(Mutex::new(None));
        assert_eq!(
            restore_keys(
                &destination,
                &backup,
                std::slice::from_ref(&proof),
                Zeroizing::new("private recovery password".into())
            ),
            Ok(1)
        );
        verify_key(&destination, &proof).unwrap();
        assert_eq!(
            restore_keys(
                &destination,
                &backup,
                std::slice::from_ref(&proof),
                Zeroizing::new("private recovery password".into())
            ),
            Ok(1)
        );
        let foreign = Memory(Mutex::new(Some((id, [91; 32]))));
        assert_eq!(
            restore_keys(
                &foreign,
                &backup,
                &[proof],
                Zeroizing::new("private recovery password".into())
            ),
            Err(Error::Conflict)
        );
        assert_eq!(*foreign.load(&id).unwrap(), [91; 32]);
    }
    #[test]
    fn bad_password_and_changed_binding_write_no_keys() {
        let id = KeyId {
            instance: Uuid::new_v4(),
            key: Uuid::new_v4(),
        };
        let source = Memory(Mutex::new(Some((id, [19; 32]))));
        let proof = prove_key(&source, &id).unwrap();
        let mut backup = backup_keys(
            &source,
            id.instance,
            vec![proof.clone()],
            Zeroizing::new("private recovery password".into()),
        )
        .unwrap();
        let destination = Memory(Mutex::new(None));
        assert_eq!(
            restore_keys(
                &destination,
                &backup,
                std::slice::from_ref(&proof),
                Zeroizing::new("incorrect password".into())
            ),
            Err(Error::Invalid)
        );
        backup.nonce[0] ^= 1;
        assert_eq!(
            restore_keys(
                &destination,
                &backup,
                &[proof],
                Zeroizing::new("private recovery password".into())
            ),
            Err(Error::Invalid)
        );
        assert!(destination.0.lock().unwrap().is_none());
    }
}
