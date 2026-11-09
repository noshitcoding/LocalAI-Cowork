use cowork_secrets::{KeyId, KeyStore};
use std::{path::Path, sync::Arc};
use zeroize::Zeroizing;

struct TeamKeys {
    id: KeyId,
    held: super::authority_token_file::PrivateValue,
}
impl KeyStore for TeamKeys {
    fn load(&self, id: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        if id != &self.id {
            return Err(cowork_secrets::Error::Missing);
        }
        let text = self.held.value().as_bytes();
        let mut key = Zeroizing::new([0u8; 32]);
        for (index, byte) in key.iter_mut().enumerate() {
            let digit = |value: u8| {
                (value as char)
                    .to_digit(16)
                    .ok_or(cowork_secrets::Error::Invalid)
            };
            *byte = ((digit(text[index * 2])? << 4) | digit(text[index * 2 + 1])?) as u8;
        }
        Ok(key)
    }
}
pub(super) fn open(data: &Path, web: Option<&Path>) -> anyhow::Result<(Arc<dyn KeyStore>, KeyId)> {
    let held = super::authority_token_file::private_value(&data.join("team-keys"), web)?;
    from_held(held)
}
pub(super) fn open_existing(
    data: &Path,
    web: Option<&Path>,
) -> anyhow::Result<(Arc<dyn KeyStore>, KeyId)> {
    from_held(super::authority_token_file::existing_private_value(
        &data.join("team-keys"),
        web,
    )?)
}
fn from_held(
    held: super::authority_token_file::PrivateValue,
) -> anyhow::Result<(Arc<dyn KeyStore>, KeyId)> {
    let id = locator(held.value())?;
    Ok((Arc::new(TeamKeys { id, held }), id))
}
pub(super) fn locator(value: &str) -> anyhow::Result<KeyId> {
    // Preserve the original text's case, never reconstruct a replacement ID.
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    context.update(b"localai-cowork:team-master-locator:1");
    context.update(value.as_bytes());
    let digest = context.finish();
    let id = KeyId {
        instance: uuid::Uuid::from_bytes(digest.as_ref()[..16].try_into()?),
        key: uuid::Uuid::from_bytes(digest.as_ref()[16..].try_into()?),
    };
    id.validate()
        .map_err(|_| anyhow::anyhow!("Encrypted team key locator is invalid"))?;
    Ok(id)
}
