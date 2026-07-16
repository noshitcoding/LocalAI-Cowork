use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[cfg(target_os = "windows")]
const SERVICE_NAME: &str = "com.open-cowork.desktop.credentials.v1";
const MAX_OWNER_BYTES: usize = 512;
const MAX_FIELD_BYTES: usize = 512;
const MAX_SECRET_BYTES: usize = 64 * 1024;
const ALLOWED_SCOPES: &[&str] = &[
    "connector",
    "crew",
    "engine",
    "llm_profile",
    "mcp_env",
    "memory_provider",
    "terminal_backend",
    "tool_gateway",
    "worker_sandbox",
];
const FRONTEND_SCOPES: &[&str] = &["connector", "crew", "engine", "llm_profile", "mcp_env"];

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialLocator {
    pub scope: String,
    pub owner_id: String,
    pub field: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSetRequest {
    #[serde(flatten)]
    pub locator: CredentialLocator,
    pub value: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialReadResponse {
    pub value: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CredentialStoreError {
    #[error("credential scope is not supported")]
    UnsupportedScope,
    #[error("credential locator is invalid")]
    InvalidLocator,
    #[error("credential value exceeds the supported size")]
    ValueTooLarge,
    #[cfg(not(target_os = "windows"))]
    #[error("operating-system credential storage is unavailable")]
    UnsupportedPlatform,
    #[error("operating-system credential storage failed")]
    Backend,
    #[error("credential storage lock is unavailable")]
    Lock,
}

trait CredentialBackend: Send + Sync {
    fn set(&self, account: &str, value: &str) -> Result<(), CredentialStoreError>;
    fn get(&self, account: &str) -> Result<Option<String>, CredentialStoreError>;
    fn delete(&self, account: &str) -> Result<(), CredentialStoreError>;
}

#[cfg(test)]
#[derive(Default)]
struct MemoryCredentialBackend {
    values: Mutex<std::collections::HashMap<String, String>>,
}

#[cfg(test)]
impl CredentialBackend for MemoryCredentialBackend {
    fn set(&self, account: &str, value: &str) -> Result<(), CredentialStoreError> {
        self.values
            .lock()
            .map_err(|_| CredentialStoreError::Lock)?
            .insert(account.to_string(), value.to_string());
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<String>, CredentialStoreError> {
        Ok(self
            .values
            .lock()
            .map_err(|_| CredentialStoreError::Lock)?
            .get(account)
            .cloned())
    }

    fn delete(&self, account: &str) -> Result<(), CredentialStoreError> {
        self.values
            .lock()
            .map_err(|_| CredentialStoreError::Lock)?
            .remove(account);
        Ok(())
    }
}

#[cfg(target_os = "windows")]
#[derive(Default)]
struct NativeCredentialBackend;

#[cfg(target_os = "windows")]
impl NativeCredentialBackend {
    fn entry(account: &str) -> Result<keyring::Entry, CredentialStoreError> {
        keyring::Entry::new(SERVICE_NAME, account).map_err(|_| CredentialStoreError::Backend)
    }
}

#[cfg(target_os = "windows")]
impl CredentialBackend for NativeCredentialBackend {
    fn set(&self, account: &str, value: &str) -> Result<(), CredentialStoreError> {
        Self::entry(account)?
            .set_password(value)
            .map_err(|_| CredentialStoreError::Backend)
    }

    fn get(&self, account: &str) -> Result<Option<String>, CredentialStoreError> {
        match Self::entry(account)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(CredentialStoreError::Backend),
        }
    }

    fn delete(&self, account: &str) -> Result<(), CredentialStoreError> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(CredentialStoreError::Backend),
        }
    }
}

#[cfg(not(target_os = "windows"))]
#[derive(Default)]
struct NativeCredentialBackend;

#[cfg(not(target_os = "windows"))]
impl CredentialBackend for NativeCredentialBackend {
    fn set(&self, _account: &str, _value: &str) -> Result<(), CredentialStoreError> {
        Err(CredentialStoreError::UnsupportedPlatform)
    }

    fn get(&self, _account: &str) -> Result<Option<String>, CredentialStoreError> {
        Err(CredentialStoreError::UnsupportedPlatform)
    }

    fn delete(&self, _account: &str) -> Result<(), CredentialStoreError> {
        Err(CredentialStoreError::UnsupportedPlatform)
    }
}

pub struct CredentialStore {
    backend: Arc<dyn CredentialBackend>,
    access_lock: Mutex<()>,
}

impl Default for CredentialStore {
    fn default() -> Self {
        Self::native()
    }
}

impl CredentialStore {
    pub fn native() -> Self {
        Self {
            backend: Arc::new(NativeCredentialBackend),
            access_lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    fn with_backend(backend: Arc<dyn CredentialBackend>) -> Self {
        Self {
            backend,
            access_lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self::with_backend(Arc::new(MemoryCredentialBackend::default()))
    }

    pub fn set(
        &self,
        locator: &CredentialLocator,
        value: &str,
    ) -> Result<(), CredentialStoreError> {
        if value.len() > MAX_SECRET_BYTES {
            return Err(CredentialStoreError::ValueTooLarge);
        }
        let account = account_id(locator)?;
        let _guard = self
            .access_lock
            .lock()
            .map_err(|_| CredentialStoreError::Lock)?;
        if value.is_empty() {
            self.backend.delete(&account)
        } else {
            self.backend.set(&account, value)
        }
    }

    pub fn get(&self, locator: &CredentialLocator) -> Result<Option<String>, CredentialStoreError> {
        let account = account_id(locator)?;
        let _guard = self
            .access_lock
            .lock()
            .map_err(|_| CredentialStoreError::Lock)?;
        let value = self.backend.get(&account)?;
        if value
            .as_ref()
            .is_some_and(|entry| entry.len() > MAX_SECRET_BYTES)
        {
            return Err(CredentialStoreError::ValueTooLarge);
        }
        Ok(value)
    }

    pub fn delete(&self, locator: &CredentialLocator) -> Result<(), CredentialStoreError> {
        let account = account_id(locator)?;
        let _guard = self
            .access_lock
            .lock()
            .map_err(|_| CredentialStoreError::Lock)?;
        self.backend.delete(&account)
    }
}

fn validate_locator_part(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && !value.chars().any(|character| character.is_control())
}

fn account_id(locator: &CredentialLocator) -> Result<String, CredentialStoreError> {
    if !ALLOWED_SCOPES.contains(&locator.scope.as_str()) {
        return Err(CredentialStoreError::UnsupportedScope);
    }
    if !validate_locator_part(&locator.owner_id, MAX_OWNER_BYTES)
        || !validate_locator_part(&locator.field, MAX_FIELD_BYTES)
    {
        return Err(CredentialStoreError::InvalidLocator);
    }

    let mut digest = Sha256::new();
    digest.update(b"open-cowork-credential-v1\0");
    digest.update(locator.scope.as_bytes());
    digest.update(b"\0");
    digest.update(locator.owner_id.as_bytes());
    digest.update(b"\0");
    digest.update(locator.field.as_bytes());
    let digest = digest.finalize();
    Ok(format!("v1-{}-{digest:x}", locator.scope))
}

pub fn validate_frontend_access(locator: &CredentialLocator) -> Result<(), CredentialStoreError> {
    if FRONTEND_SCOPES.contains(&locator.scope.as_str()) {
        Ok(())
    } else {
        Err(CredentialStoreError::UnsupportedScope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locator() -> CredentialLocator {
        CredentialLocator {
            scope: "llm_profile".to_string(),
            owner_id: "profile-owner@example.test".to_string(),
            field: "api_key".to_string(),
        }
    }

    #[test]
    fn account_ids_are_stable_and_do_not_expose_locator_values() {
        let first = account_id(&locator()).expect("account id");
        let second = account_id(&locator()).expect("account id");

        assert_eq!(first, second);
        assert!(first.starts_with("v1-llm_profile-"));
        assert!(!first.contains("profile-owner"));
        assert!(!first.contains("api_key"));
    }

    #[test]
    fn store_round_trips_and_deletes_credentials() {
        let store = CredentialStore::in_memory();
        let locator = locator();

        assert_eq!(store.get(&locator), Ok(None));
        store.set(&locator, "top-secret-value").expect("set");
        assert_eq!(
            store.get(&locator),
            Ok(Some("top-secret-value".to_string()))
        );
        store.delete(&locator).expect("delete");
        assert_eq!(store.get(&locator), Ok(None));
    }

    #[test]
    fn empty_values_delete_existing_credentials() {
        let store = CredentialStore::in_memory();
        let locator = locator();

        store.set(&locator, "temporary").expect("set");
        store.set(&locator, "").expect("empty set deletes");
        assert_eq!(store.get(&locator), Ok(None));
    }

    #[test]
    fn locator_and_size_validation_fail_closed() {
        let store = CredentialStore::in_memory();
        let mut invalid_scope = locator();
        invalid_scope.scope = "arbitrary".to_string();
        assert_eq!(
            store.get(&invalid_scope),
            Err(CredentialStoreError::UnsupportedScope)
        );

        let mut invalid_owner = locator();
        invalid_owner.owner_id = "line\nbreak".to_string();
        assert_eq!(
            store.get(&invalid_owner),
            Err(CredentialStoreError::InvalidLocator)
        );

        let oversized = "x".repeat(MAX_SECRET_BYTES + 1);
        assert_eq!(
            store.set(&locator(), &oversized),
            Err(CredentialStoreError::ValueTooLarge)
        );
    }

    #[test]
    fn backend_only_scopes_are_rejected_at_the_frontend_ipc_boundary() {
        let backend_locator = CredentialLocator {
            scope: "terminal_backend".to_string(),
            owner_id: "backend-1".to_string(),
            field: "config_blob:550e8400-e29b-41d4-a716-446655440000".to_string(),
        };
        assert_eq!(
            validate_frontend_access(&backend_locator),
            Err(CredentialStoreError::UnsupportedScope)
        );
        assert_eq!(validate_frontend_access(&locator()), Ok(()));
    }

    #[test]
    fn errors_never_format_secret_values() {
        let store = CredentialStore::in_memory();
        let secret = "unique-secret-that-must-not-be-logged".repeat(4096);
        let error = store
            .set(&locator(), &secret)
            .expect_err("oversized secret is rejected");

        assert!(!error.to_string().contains("unique-secret"));
        assert!(!format!("{error:?}").contains("unique-secret"));
    }
}
