//! Native authentication request persistence uses only explicitly admitted native keys.
use super::*;
use cowork_secrets::{KeyId, KeyStore, load_verified_key};
use zeroize::Zeroizing;
struct JournalKeys(Vec<(KeyId, Zeroizing<[u8; 32]>)>);
impl KeyStore for JournalKeys {
    fn load(&self, id: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        self.0
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, v)| Zeroizing::new(**v))
            .ok_or(cowork_secrets::Error::Missing)
    }
}
impl Runtime {
    async fn auth_transport_keys(
        &self,
    ) -> Result<(cowork_storage::NativeSecretState, JournalKeys)> {
        let provider = self.native_key_provider()?;
        let (state, proofs) = self.store.native_recovery_inventory().await?;
        let instance = state.instance;
        let keys = crate::native_secrets::native_call(move || {
            let manager = provider.open(instance)?;
            let mut values = Vec::new();
            for proof in proofs {
                values.push((proof.key, load_verified_key(manager.as_ref(), &proof)?));
            }
            Ok(JournalKeys(values))
        })
        .await?;
        Ok((state, keys))
    }
    pub async fn native_auth_transport_pending(&self) -> Result<Vec<NativeAuthTransportView>> {
        self.native_key_provider()?;
        self.store.native_auth_transport_pending().await
    }
    pub async fn native_auth_transport_prepare(
        &self,
        v: &NativeAuthTransportRequest,
    ) -> Result<NativeAuthTransportView> {
        let (state, keys) = self.auth_transport_keys().await?;
        self.store
            .native_auth_transport_prepare(&keys, &state, v)
            .await
    }
    pub async fn native_auth_transport_read(
        &self,
        id: uuid::Uuid,
    ) -> Result<NativeAuthTransportReplay> {
        let (state, keys) = self.auth_transport_keys().await?;
        self.store
            .native_auth_transport_read(&keys, &state, id)
            .await
    }
    pub async fn native_auth_transport_capture(
        &self,
        v: &NativeAuthTransportCapture,
    ) -> Result<NativeAuthTransportView> {
        let (state, keys) = self.auth_transport_keys().await?;
        self.store
            .native_auth_transport_capture(&keys, &state, v.operation_id, &v.response_json)
            .await
    }
    pub async fn native_auth_transport_resolve(
        &self,
        v: &NativeAuthTransportResolution,
    ) -> Result<NativeAuthTransportView> {
        self.native_key_provider()?;
        self.store
            .native_auth_transport_finish(v.operation_id, v.acknowledged)
            .await
    }
}
