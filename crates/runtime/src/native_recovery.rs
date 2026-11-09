use super::*;
use cowork_secrets::{KeyBackup, KeyId, KeyStore, load_verified_key};
use zeroize::Zeroizing;

struct RecoveryKeys(Vec<(KeyId, Zeroizing<[u8; 32]>)>);
impl KeyStore for RecoveryKeys {
    fn load(&self, id: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        self.0
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, bytes)| Zeroizing::new(**bytes))
            .ok_or(cowork_secrets::Error::Missing)
    }
}
impl Runtime {
    pub async fn native_key_cleanup(&self) -> Result<Value> {
        self.native_key_provider()?;
        self.store.native_key_cleanup_view().await
    }
    pub async fn retire_native_key(
        &self,
        expected: i64,
        key: KeyId,
        request: &str,
    ) -> Result<Value> {
        let provider = self.native_key_provider()?;
        let receipt = self
            .store
            .begin_native_key_retirement(expected, key, request)
            .await?;
        if receipt.state != "removed" {
            let proof = receipt.proof.clone().ok_or(Error::Forbidden)?;
            crate::native_secrets::native_call(move || provider.open(key.instance)?.retire(&proof))
                .await
                .map_err(|_| Error::UncertainEffect)?;
        }
        serde_json::to_value(self.store.finish_native_key_retirement(&receipt).await?)
            .map_err(storage_error)
    }
    pub async fn retire_native_orphan_key(
        &self,
        expected: i64,
        operation_id: &str,
        request: &str,
    ) -> Result<Value> {
        let provider = self.native_key_provider()?;
        let original = self.store.native_key_operation(operation_id).await?;
        if original.state != NativeSecretsSetupState::Abandoned || original.proof.is_some() {
            return Err(Error::Forbidden);
        }
        let key = original.key;
        let inspect = provider.clone();
        let observed = crate::native_secrets::native_call(move || {
            let manager = inspect.open(key.instance)?;
            match cowork_secrets::prove_key(manager.as_ref(), &key) {
                Ok(proof) => Ok(Some(proof)),
                Err(cowork_secrets::Error::Missing) => Ok(None),
                Err(error) => Err(error),
            }
        })
        .await?;
        let receipt = self
            .store
            .begin_native_orphan_retirement(expected, operation_id, request, observed.as_ref())
            .await?;
        if receipt.state != "removed" {
            let proof = receipt.proof.clone();
            crate::native_secrets::native_call(move || {
                let manager = provider.open(key.instance)?;
                if let Some(proof) = proof {
                    manager.retire(&proof)
                } else {
                    match manager.load(&key) {
                        Err(cowork_secrets::Error::Missing) => Ok(()),
                        Ok(_) => Err(cowork_secrets::Error::Conflict),
                        Err(error) => Err(error),
                    }
                }
            })
            .await
            .map_err(|_| Error::UncertainEffect)?;
        }
        serde_json::to_value(self.store.finish_native_key_retirement(&receipt).await?)
            .map_err(storage_error)
    }
    pub async fn backup_native_keys(
        &self,
        expected: i64,
        password: Zeroizing<String>,
    ) -> Result<KeyBackup> {
        let provider = self.native_key_provider()?;
        let (state, proofs) = self.store.native_recovery_inventory().await?;
        if state.revision != expected {
            return Err(Error::Conflict);
        }
        let backup = crate::native_secrets::native_call(move || {
            let manager = provider.open(state.instance)?;
            cowork_secrets::backup_keys(manager.as_ref(), state.instance, proofs, password)
        })
        .await?;
        if self
            .store
            .native_secret_state()
            .await?
            .is_none_or(|s| s.revision != expected || s.pending_operation.is_some())
        {
            return Err(Error::Conflict);
        }
        Ok(backup)
    }
    pub async fn restore_native_keys(
        &self,
        expected: i64,
        backup: KeyBackup,
        password: Zeroizing<String>,
    ) -> Result<Value> {
        let provider = self.native_key_provider()?;
        let guard = self
            .store
            .begin_native_key_restore(expected, &backup)
            .await?;
        let (count, guard) = crate::native_secrets::native_call(move || {
            let manager = provider.open(guard.instance())?;
            let count =
                cowork_secrets::restore_keys(manager.as_ref(), &backup, guard.proofs(), password)?;
            Ok((count, guard))
        })
        .await
        .map_err(|_| Error::UncertainEffect)?;
        guard.finish().await?;
        let status = self.native_secrets_status().await?;
        Ok(json!({"state":"restored","key_count":count,"store":status}))
    }
    pub async fn reencrypt_native_keys(&self, expected: i64) -> Result<Value> {
        let provider = self.native_key_provider()?;
        let (state, proofs) = self.store.native_recovery_inventory().await?;
        if state.revision != expected {
            return Err(Error::Conflict);
        }
        let keys = crate::native_secrets::native_call(move || {
            let manager = provider.open(state.instance)?;
            let mut keys = Vec::new();
            for proof in proofs {
                keys.push((proof.key, load_verified_key(manager.as_ref(), &proof)?));
            }
            Ok(RecoveryKeys(keys))
        })
        .await?;
        self.store.reencrypt_native_secrets(expected, &keys).await
    }
}
