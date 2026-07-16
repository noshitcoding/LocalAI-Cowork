use crate::credential_store::{CredentialLocator, CredentialStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const REFERENCE_KEY: &str = "$openCoworkCredential";
const REFERENCE_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureConfigScope {
    MemoryProvider,
    TerminalBackend,
    ToolGateway,
    WorkerSandbox,
}

impl SecureConfigScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MemoryProvider => "memory_provider",
            Self::TerminalBackend => "terminal_backend",
            Self::ToolGateway => "tool_gateway",
            Self::WorkerSandbox => "worker_sandbox",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecureConfigReference {
    version: u8,
    scope: String,
    revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecureConfigEnvelope {
    #[serde(rename = "$openCoworkCredential")]
    reference: SecureConfigReference,
}

struct PreparedSecureConfig {
    marker_json: String,
    locator: CredentialLocator,
}

fn locator(scope: SecureConfigScope, owner_id: &str, revision: &str) -> CredentialLocator {
    CredentialLocator {
        scope: scope.as_str().to_string(),
        owner_id: owner_id.to_string(),
        field: format!("config_blob:{revision}"),
    }
}

fn parse_reference(
    stored_value: &str,
    expected_scope: SecureConfigScope,
    owner_id: &str,
) -> Result<Option<CredentialLocator>, String> {
    let Ok(value) = serde_json::from_str::<Value>(stored_value) else {
        return Ok(None);
    };
    let contains_reference = value
        .as_object()
        .is_some_and(|object| object.contains_key(REFERENCE_KEY));
    if !contains_reference {
        return Ok(None);
    }

    let envelope: SecureConfigEnvelope = serde_json::from_value(value)
        .map_err(|_| "secure configuration reference is invalid".to_string())?;
    if envelope.reference.version != REFERENCE_VERSION
        || envelope.reference.scope != expected_scope.as_str()
        || uuid::Uuid::parse_str(&envelope.reference.revision).is_err()
    {
        return Err("secure configuration reference is invalid".to_string());
    }
    Ok(Some(locator(
        expected_scope,
        owner_id,
        &envelope.reference.revision,
    )))
}

fn prepare(
    store: &CredentialStore,
    scope: SecureConfigScope,
    owner_id: &str,
    plaintext: &str,
) -> Result<PreparedSecureConfig, String> {
    let revision = uuid::Uuid::new_v4().to_string();
    let locator = locator(scope, owner_id, &revision);
    store
        .set(&locator, plaintext)
        .map_err(|error| error.to_string())?;
    let marker_json = serde_json::to_string(&SecureConfigEnvelope {
        reference: SecureConfigReference {
            version: REFERENCE_VERSION,
            scope: scope.as_str().to_string(),
            revision,
        },
    });
    let marker_json = match marker_json {
        Ok(value) => value,
        Err(_) => {
            let _ = store.delete(&locator);
            return Err("secure configuration reference could not be created".to_string());
        }
    };
    Ok(PreparedSecureConfig {
        marker_json,
        locator,
    })
}

pub fn validate_json_document(value: &str) -> Result<(), String> {
    serde_json::from_str::<Value>(value)
        .map(|_| ())
        .map_err(|_| "configuration must be valid JSON".to_string())
}

pub fn is_reference(value: &str, scope: SecureConfigScope, owner_id: &str) -> Result<bool, String> {
    parse_reference(value, scope, owner_id).map(|reference| reference.is_some())
}

pub fn resolve(
    store: &CredentialStore,
    scope: SecureConfigScope,
    owner_id: &str,
    stored_value: &str,
) -> Result<String, String> {
    let Some(locator) = parse_reference(stored_value, scope, owner_id)? else {
        return Ok(stored_value.to_string());
    };
    store
        .get(&locator)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "secure configuration value is missing".to_string())
}

pub fn replace<F>(
    store: &CredentialStore,
    scope: SecureConfigScope,
    owner_id: &str,
    plaintext: &str,
    previous_stored_value: Option<&str>,
    commit: F,
) -> Result<String, String>
where
    F: FnOnce(&str) -> Result<(), String>,
{
    let previous_locator = previous_stored_value
        .map(|value| parse_reference(value, scope, owner_id))
        .transpose()?
        .flatten();
    let prepared = prepare(store, scope, owner_id, plaintext)?;
    if let Err(error) = commit(&prepared.marker_json) {
        let _ = store.delete(&prepared.locator);
        return Err(error);
    }
    if let Some(previous_locator) = previous_locator {
        if previous_locator.field != prepared.locator.field {
            let _ = store.delete(&previous_locator);
        }
    }
    Ok(prepared.marker_json)
}

pub fn delete_reference(
    store: &CredentialStore,
    scope: SecureConfigScope,
    owner_id: &str,
    stored_value: Option<&str>,
) -> Result<(), String> {
    let Some(stored_value) = stored_value else {
        return Ok(());
    };
    if let Some(locator) = parse_reference(stored_value, scope, owner_id)? {
        store.delete(&locator).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referenced_config_round_trips_without_exposing_owner_or_value() {
        let store = CredentialStore::in_memory();
        let secret_json = r#"{"unknownField":"unstructured-secret"}"#;
        let mut committed = String::new();

        let marker = replace(
            &store,
            SecureConfigScope::ToolGateway,
            "gateway-owner-id",
            secret_json,
            None,
            |value| {
                committed = value.to_string();
                Ok(())
            },
        )
        .expect("configuration should be protected");

        assert_eq!(marker, committed);
        assert!(!marker.contains("gateway-owner-id"));
        assert!(!marker.contains("unstructured-secret"));
        assert_eq!(
            resolve(
                &store,
                SecureConfigScope::ToolGateway,
                "gateway-owner-id",
                &marker,
            ),
            Ok(secret_json.to_string())
        );
    }

    #[test]
    fn failed_commit_preserves_previous_revision() {
        let store = CredentialStore::in_memory();
        let mut previous_marker = String::new();
        replace(
            &store,
            SecureConfigScope::TerminalBackend,
            "backend-1",
            r#"{"envVars":{"TOKEN":"old-secret"}}"#,
            None,
            |value| {
                previous_marker = value.to_string();
                Ok(())
            },
        )
        .expect("first revision");

        let result = replace(
            &store,
            SecureConfigScope::TerminalBackend,
            "backend-1",
            r#"{"envVars":{"TOKEN":"new-secret"}}"#,
            Some(&previous_marker),
            |_| Err("database write failed".to_string()),
        );

        assert_eq!(result, Err("database write failed".to_string()));
        assert_eq!(
            resolve(
                &store,
                SecureConfigScope::TerminalBackend,
                "backend-1",
                &previous_marker,
            ),
            Ok(r#"{"envVars":{"TOKEN":"old-secret"}}"#.to_string())
        );
    }

    #[test]
    fn malformed_or_cross_scope_references_fail_closed() {
        let store = CredentialStore::in_memory();
        let malformed = r#"{"$openCoworkCredential":{"version":1,"scope":"tool_gateway","revision":"not-a-uuid"}}"#;
        assert!(resolve(&store, SecureConfigScope::ToolGateway, "owner", malformed,).is_err());

        let cross_scope = r#"{"$openCoworkCredential":{"version":1,"scope":"memory_provider","revision":"550e8400-e29b-41d4-a716-446655440000"}}"#;
        assert!(resolve(&store, SecureConfigScope::ToolGateway, "owner", cross_scope,).is_err());
    }

    #[test]
    fn deleting_a_reference_removes_its_resolvable_value() {
        let store = CredentialStore::in_memory();
        let mut marker = String::new();
        replace(
            &store,
            SecureConfigScope::MemoryProvider,
            "provider-delete",
            r#"{"secret":"delete-me"}"#,
            None,
            |value| {
                marker = value.to_string();
                Ok(())
            },
        )
        .expect("configuration should be protected");

        delete_reference(
            &store,
            SecureConfigScope::MemoryProvider,
            "provider-delete",
            Some(&marker),
        )
        .expect("reference should be deleted");

        assert_eq!(
            resolve(
                &store,
                SecureConfigScope::MemoryProvider,
                "provider-delete",
                &marker,
            ),
            Err("secure configuration value is missing".to_string())
        );
    }

    #[test]
    fn non_reference_legacy_values_are_returned_for_migration() {
        let store = CredentialStore::in_memory();
        assert_eq!(
            resolve(
                &store,
                SecureConfigScope::MemoryProvider,
                "provider-1",
                r#"{"apiKey":"legacy-secret"}"#,
            ),
            Ok(r#"{"apiKey":"legacy-secret"}"#.to_string())
        );
    }
}
