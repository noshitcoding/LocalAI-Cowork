//! Read-only compatibility with V1's fixed Windows credential. Never create,
//! replace, delete, export, or accept a renderer-supplied signing key.
use crate::{Error, Result};
use zeroize::Zeroizing;

pub struct NativeV1AuditKey(Zeroizing<[u8; 32]>);
impl NativeV1AuditKey {
    pub fn load() -> Result<Self> {
        load()
    }
    pub fn verify(&self, bytes: &[u8], mac: &[u8]) -> bool {
        ring::hmac::verify(
            &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, self.0.as_ref()),
            bytes,
            mac,
        )
        .is_ok()
    }
}
#[cfg(not(windows))]
fn load() -> Result<NativeV1AuditKey> {
    Err(Error::Unavailable)
}

#[cfg(windows)]
fn load() -> Result<NativeV1AuditKey> {
    use windows_sys::Win32::{
        Foundation::{ERROR_NOT_FOUND, GetLastError},
        Security::Credentials::*,
    };
    use zeroize::Zeroize;
    const ACCOUNT: &str = "audit-signing-key-v1";
    // keyring 3.6.3 Windows maps Entry::new(service,user) to user.service and
    // stores set_password as UTF16LE, with ENTERPRISE persistence in V1.
    const TARGET: &str = "audit-signing-key-v1.com.open-cowork.desktop.audit.v1";
    struct Owned(*mut CREDENTIALW);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                let c = &mut *self.0;
                if !c.CredentialBlob.is_null()
                    && c.CredentialBlobSize <= CRED_MAX_CREDENTIAL_BLOB_SIZE
                {
                    std::slice::from_raw_parts_mut(c.CredentialBlob, c.CredentialBlobSize as usize)
                        .zeroize();
                }
                CredFree(self.0.cast());
            }
        }
    }
    unsafe fn name(pointer: *const u16, expected: &str) -> bool {
        if pointer.is_null() {
            return false;
        }
        for (i, c) in expected.encode_utf16().chain(Some(0)).enumerate() {
            if unsafe { *pointer.add(i) } != c {
                return false;
            }
        }
        true
    }
    let target: Vec<u16> = TARGET.encode_utf16().chain(Some(0)).collect();
    let mut ptr = std::ptr::null_mut();
    if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut ptr) } == 0 {
        return Err(if unsafe { GetLastError() } == ERROR_NOT_FOUND {
            Error::Missing
        } else {
            Error::Unavailable
        });
    }
    if ptr.is_null() {
        return Err(Error::Invalid);
    }
    let owned = Owned(ptr);
    let c = unsafe { &*owned.0 };
    if c.Type != CRED_TYPE_GENERIC
        || !matches!(
            c.Persist,
            CRED_PERSIST_LOCAL_MACHINE | CRED_PERSIST_ENTERPRISE
        )
        || c.CredentialBlobSize != 128
        || c.CredentialBlob.is_null()
        || !unsafe { name(c.TargetName, TARGET) }
        || !unsafe { name(c.UserName, ACCOUNT) }
    {
        return Err(Error::Invalid);
    }
    let bytes = unsafe { std::slice::from_raw_parts(c.CredentialBlob, 128) };
    let mut key = Zeroizing::new([0u8; 32]);
    fn nibble(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        }
    }
    for i in 0..32 {
        if bytes[i * 4 + 1] != 0 || bytes[i * 4 + 3] != 0 {
            return Err(Error::Invalid);
        }
        key[i] = nibble(bytes[i * 4]).ok_or(Error::Invalid)? * 16
            + nibble(bytes[i * 4 + 2]).ok_or(Error::Invalid)?;
    }
    Ok(NativeV1AuditKey(key))
}
