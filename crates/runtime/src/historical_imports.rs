//! Native-personal M9 uses the actual OS-proved native key lifecycle.
use super::*;
use cowork_secrets::{KeyId, KeyStore, load_verified_key};
use cowork_storage::historical_imports::product::{
    HistoricalKeySelection, NativeHistoricalAuthority,
};
use zeroize::Zeroizing;
struct HistoricalNativeKeys(Vec<(KeyId, Zeroizing<[u8; 32]>)>);
impl KeyStore for HistoricalNativeKeys {
    fn load(&self, id: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        self.0
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, material)| Zeroizing::new(**material))
            .ok_or(cowork_secrets::Error::Missing)
    }
}
pub struct NativeHistoricalServices {
    pub authority: NativeHistoricalAuthority,
    pub keys: Arc<dyn KeyStore>,
    pub key_id: KeyId,
}
impl Runtime {
    pub async fn native_historical_keys(
        &self,
        selection: HistoricalKeySelection,
    ) -> Result<NativeHistoricalServices> {
        let provider = self.native_key_provider()?;
        let (state, proofs) = self.store.native_historical_inventory(selection).await?;
        let instance = state.instance;
        let keys = crate::native_secrets::native_call(move || {
            let manager = provider.open(instance)?;
            let mut keys = vec![];
            for proof in proofs {
                keys.push((proof.key, load_verified_key(manager.as_ref(), &proof)?));
            }
            Ok(HistoricalNativeKeys(keys))
        })
        .await?;
        let authority = NativeHistoricalAuthority::from_current_runtime_state(state)?;
        let key_id = authority.key_id()?;
        Ok(NativeHistoricalServices {
            authority,
            keys: Arc::new(keys),
            key_id,
        })
    }
}
