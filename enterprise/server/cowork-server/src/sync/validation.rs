//! Sync protocol validation, secret rejection and canonical payload field validation.

use crate::error::ApiError;
use chrono::{Duration, Utc};
use cowork_contracts::{ensure_compatible, SyncChange, SyncOperation};
use serde_json::Value;
use uuid::Uuid;

pub(in crate::sync) const MAX_SYNC_BATCH: usize = 100;

pub(in crate::sync) const MAX_SYNC_PAYLOAD_BYTES: usize = 512 * 1024;

const ALLOWED_ENTITY_TYPES: &[&str] = &[
    "project",
    "thread",
    "message",
    "task",
    "schedule",
    "run",
    "crew",
    "skill",
    "memory",
    "provider_profile",
    "secret_metadata",
    "mcp_metadata",
];

pub(in crate::sync) fn validate_change(
    change: &SyncChange,
    session_device_id: Uuid,
) -> Result<(), ApiError> {
    ensure_compatible(change.schema_version)
        .map_err(|error| ApiError::Unprocessable(error.to_string()))?;
    if change.device_id != session_device_id {
        return Err(ApiError::Unauthorized(
            "sync change device_id does not match the authenticated session".to_owned(),
        ));
    }
    validate_entity_type(&change.entity_type)?;
    if change.base_revision < 0 {
        return Err(ApiError::Unprocessable(
            "base_revision must not be negative".to_owned(),
        ));
    }
    match change.operation {
        SyncOperation::Upsert if change.payload.is_none() => {
            return Err(ApiError::Unprocessable(
                "upsert changes require a payload".to_owned(),
            ))
        }
        SyncOperation::Delete if change.payload.is_some() => {
            return Err(ApiError::Unprocessable(
                "delete changes must not contain a payload".to_owned(),
            ))
        }
        _ => {}
    }
    if change.payload.as_ref().is_some_and(|payload| {
        serde_json::to_vec(payload).is_ok_and(|bytes| bytes.len() > MAX_SYNC_PAYLOAD_BYTES)
    }) {
        return Err(ApiError::Unprocessable(format!(
            "sync payload must not exceed {MAX_SYNC_PAYLOAD_BYTES} bytes"
        )));
    }
    if matches!(
        change.entity_type.as_str(),
        "provider_profile" | "secret_metadata" | "mcp_metadata"
    ) && change
        .payload
        .as_ref()
        .is_some_and(contains_cleartext_secret_field)
    {
        return Err(ApiError::Unprocessable(
            "metadata sync must not contain cleartext secret fields".to_owned(),
        ));
    }
    if change.client_timestamp > Utc::now() + Duration::minutes(5) {
        return Err(ApiError::Unprocessable(
            "client_timestamp is too far in the future".to_owned(),
        ));
    }
    Ok(())
}

pub(in crate::sync) fn validate_entity_type(entity_type: &str) -> Result<(), ApiError> {
    if ALLOWED_ENTITY_TYPES.contains(&entity_type) {
        Ok(())
    } else {
        Err(ApiError::Unprocessable(format!(
            "unsupported sync entity type {entity_type}"
        )))
    }
}

fn contains_cleartext_secret_field(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            matches!(
                key.to_ascii_lowercase().as_str(),
                "api_key"
                    | "access_token"
                    | "refresh_token"
                    | "password"
                    | "secret"
                    | "client_secret"
                    | "authorization"
            ) || contains_cleartext_secret_field(value)
        }),
        Value::Array(items) => items.iter().any(contains_cleartext_secret_field),
        _ => false,
    }
}

pub(in crate::sync) fn object_payload<'a>(
    payload: Option<&'a Value>,
    entity_type: &str,
) -> Result<&'a serde_json::Map<String, Value>, ApiError> {
    payload.and_then(Value::as_object).ok_or_else(|| {
        ApiError::Unprocessable(format!("{entity_type} sync payload must be a JSON object"))
    })
}

pub(in crate::sync) fn required_text(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
    label: &str,
    maximum: usize,
) -> Result<String, ApiError> {
    let value = keys
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .trim();
    if value.is_empty() || value.len() > maximum {
        return Err(ApiError::Unprocessable(format!(
            "{label} must contain 1 to {maximum} characters"
        )));
    }
    Ok(value.to_owned())
}

pub(in crate::sync) fn optional_text(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
    maximum: usize,
) -> Result<String, ApiError> {
    let value = keys
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .unwrap_or_default();
    if value.len() > maximum {
        return Err(ApiError::Unprocessable(format!(
            "metadata text must not exceed {maximum} characters"
        )));
    }
    Ok(value.to_owned())
}

pub(in crate::sync) fn required_uuid(
    object: &serde_json::Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<Uuid, ApiError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::Unprocessable(format!("{label} must be a UUID string")))
        .and_then(|value| {
            Uuid::parse_str(value)
                .map_err(|_| ApiError::Unprocessable(format!("{label} must be a UUID string")))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowork_contracts::SCHEMA_VERSION;

    fn change(operation: SyncOperation, payload: Option<Value>) -> SyncChange {
        SyncChange {
            schema_version: SCHEMA_VERSION,
            operation_id: Uuid::new_v4(),
            device_id: Uuid::nil(),
            entity_type: "memory".to_owned(),
            entity_id: Uuid::new_v4(),
            base_revision: 0,
            operation,
            payload,
            client_timestamp: Utc::now(),
        }
    }

    #[test]
    fn validates_payload_and_device_boundaries() {
        assert!(validate_change(
            &change(
                SyncOperation::Upsert,
                Some(serde_json::json!({"text": "ok"}))
            ),
            Uuid::nil()
        )
        .is_ok());
        assert!(validate_change(&change(SyncOperation::Upsert, None), Uuid::nil()).is_err());
        assert!(validate_change(
            &change(SyncOperation::Delete, Some(serde_json::json!({}))),
            Uuid::nil()
        )
        .is_err());
        assert!(validate_change(&change(SyncOperation::Delete, None), Uuid::new_v4()).is_err());
    }

    #[test]
    fn rejects_cleartext_provider_secrets_but_allows_presence_metadata() {
        let mut safe = change(
            SyncOperation::Upsert,
            Some(serde_json::json!({"name": "OpenAI", "has_api_key": true})),
        );
        safe.entity_type = "provider_profile".to_owned();
        assert!(validate_change(&safe, Uuid::nil()).is_ok());
        safe.payload = Some(serde_json::json!({"api_key": "must-not-sync"}));
        assert!(validate_change(&safe, Uuid::nil()).is_err());
    }
}
