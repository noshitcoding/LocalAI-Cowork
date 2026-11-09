use super::*;
pub(super) struct Entry(keyring::Entry);
impl Entry {
    pub fn new(service: &str, user: &str) -> Result<Self> {
        let credential = keyring::secret_service::default_credential_builder()
            .build(None, service, user)
            .map_err(map_error)?;
        Ok(Self(keyring::Entry::new_with_credential(credential)))
    }
    pub fn get_secret(&self) -> Result<Vec<u8>> {
        self.0.get_secret().map_err(map_error)
    }
    pub fn set_secret(&self, secret: &[u8]) -> Result<()> {
        self.0.set_secret(secret).map_err(map_error)
    }
    pub fn delete_credential(&self) -> Result<()> {
        self.0.delete_credential().map_err(map_error)
    }
}
fn map_error(error: keyring::Error) -> Error {
    // Never format a backend error: its payload may include secret material.
    match error {
        keyring::Error::NoEntry => Error::Missing,
        keyring::Error::Ambiguous(_) => Error::Conflict,
        keyring::Error::BadEncoding(bytes) => {
            drop(Zeroizing::new(bytes));
            Error::Invalid
        }
        _ => Error::Unavailable,
    }
}
