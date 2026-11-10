//! Conversions between durable sync rows and the existing sync wire records.

use crate::error::ApiError;
use cowork_contracts::{ServerSyncChange, SyncOperation, SyncedEntity, SCHEMA_VERSION};
use sqlx::{postgres::PgRow, Row};

pub(in crate::sync) fn row_to_synced_entity(row: &PgRow) -> Result<SyncedEntity, ApiError> {
    Ok(SyncedEntity {
        schema_version: SCHEMA_VERSION,
        entity_type: row.try_get("entity_type")?,
        entity_id: row.try_get("entity_id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        payload: row.try_get("payload")?,
        tombstone: row.try_get("tombstone")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub(in crate::sync) fn row_to_server_change(row: &PgRow) -> Result<ServerSyncChange, ApiError> {
    Ok(ServerSyncChange {
        schema_version: SCHEMA_VERSION,
        cursor: row.try_get("cursor")?,
        entity_type: row.try_get("entity_type")?,
        entity_id: row.try_get("entity_id")?,
        revision: row.try_get("revision")?,
        operation: parse_operation(row.try_get("operation")?)?,
        payload: row.try_get("payload")?,
        created_at: row.try_get("created_at")?,
    })
}

pub(in crate::sync) fn operation_name(operation: SyncOperation) -> &'static str {
    match operation {
        SyncOperation::Upsert => "upsert",
        SyncOperation::Delete => "delete",
    }
}

pub(in crate::sync) fn parse_operation(operation: &str) -> Result<SyncOperation, ApiError> {
    match operation {
        "upsert" => Ok(SyncOperation::Upsert),
        "delete" => Ok(SyncOperation::Delete),
        other => Err(ApiError::Internal(anyhow::anyhow!(
            "unknown sync operation {other}"
        ))),
    }
}
