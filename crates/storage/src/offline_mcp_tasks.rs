//! Local signed-grant MCP tasks. The OfflineDevice vault is a separate key and
//! authority domain from NativeState, identity-private Actors and team servers.
use super::*;
use cowork_secrets::KeyId;
use sqlx::{Any, Transaction};
mod calls;
mod interactions;
mod receipts;
mod source;
pub(crate) use interactions::{control_current, decision_current};
pub use source::OfflineMcpCallPermit;

/// Receipt encryption may outlive current execution authority, but may never
/// substitute a Server/NativeState key or another installation's OfflineDevice.
pub(crate) async fn cipher_writer(tx: &mut Transaction<'_, Any>, key: KeyId) -> Result<()> {
    key.validate().map_err(|_| Error::NativeKeyUnavailable)?;
    let raw =
        sqlx::query_scalar::<_, String>("SELECT payload FROM offline_local_device WHERE id=1")
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NativeKeyUnavailable)?;
    let device: crate::offline_execution::OfflineLocalDeviceRecord = parse(&raw)?;
    if device.key_id != key
        || device.device.id != key.instance
        || device.device.public_key == "pending"
    {
        return Err(Error::NativeKeyUnavailable);
    }
    Ok(())
}
