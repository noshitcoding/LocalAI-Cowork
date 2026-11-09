use super::*;
use windows_sys::Win32::{
    Foundation::{ERROR_NOT_FOUND, GetLastError},
    Security::Credentials::*,
};
use zeroize::Zeroize;

pub(super) struct Entry {
    target: Vec<u16>,
}
impl Entry {
    pub fn new(service: &str, user: &str) -> Result<Self> {
        Ok(Self {
            target: format!("{service}:{user}")
                .encode_utf16()
                .chain(Some(0))
                .collect(),
        })
    }
    pub fn get_secret(&self) -> Result<Vec<u8>> {
        let mut pointer = std::ptr::null_mut();
        // CredReadW allocates one owned block; OwnedCredential frees every path.
        if unsafe { CredReadW(self.target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut pointer) } == 0 {
            return Err(last_error());
        }
        if pointer.is_null() {
            return Err(Error::Invalid);
        }
        let owned = OwnedCredential(pointer);
        let record = unsafe { &*owned.0 };
        if record.Type != CRED_TYPE_GENERIC
            || record.Persist != CRED_PERSIST_LOCAL_MACHINE
            || record.CredentialBlobSize != 72
            || record.CredentialBlob.is_null()
        {
            return Err(Error::Invalid);
        }
        Ok(unsafe { std::slice::from_raw_parts(record.CredentialBlob, 72) }.to_vec())
    }
    pub fn set_secret(&self, secret: &[u8]) -> Result<()> {
        if secret.len() != 72 {
            return Err(Error::Invalid);
        }
        let mut bytes = Zeroizing::new(secret.to_vec());
        let mut target = self.target.clone();
        let mut user: Vec<u16> = "LocalAI Cowork V2".encode_utf16().chain(Some(0)).collect();
        let record = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            CredentialBlobSize: bytes.len() as u32,
            CredentialBlob: bytes.as_mut_ptr(),
            // Explicitly machine-local. The keyring crate's Windows adapter uses
            // ENTERPRISE persistence, which could roam keys without this choice.
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: user.as_mut_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        if unsafe { CredWriteW(&record, 0) } == 0 {
            return Err(last_error());
        }
        Ok(())
    }
    pub fn delete_credential(&self) -> Result<()> {
        if unsafe { CredDeleteW(self.target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            return Err(last_error());
        }
        Ok(())
    }
}
struct OwnedCredential(*mut CREDENTIALW);
impl Drop for OwnedCredential {
    fn drop(&mut self) {
        unsafe {
            let record = &mut *self.0;
            if !record.CredentialBlob.is_null()
                && record.CredentialBlobSize <= CRED_MAX_CREDENTIAL_BLOB_SIZE
            {
                std::slice::from_raw_parts_mut(
                    record.CredentialBlob,
                    record.CredentialBlobSize as usize,
                )
                .zeroize();
            }
            CredFree(self.0.cast());
        }
    }
}
fn last_error() -> Error {
    if unsafe { GetLastError() } == ERROR_NOT_FOUND {
        Error::Missing
    } else {
        Error::Unavailable
    }
}
