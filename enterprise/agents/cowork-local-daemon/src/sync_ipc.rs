use anyhow::{bail, Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use uuid::Uuid;

use super::Daemon;

const MAX_PEER_ID_BYTES: usize = 2_048;
const MAX_SYNC_PAYLOAD_BYTES: usize = 512 * 1024;

pub async fn state(daemon: &Daemon, params: Value) -> Result<Value> {
    let peer_id = peer_id(&params)?;
    let database = daemon.database.lock().await;
    ensure_schema(&database)?;
    ensure_peer(&database, peer_id)?;
    let (local_cursor, remote_cursor) = database.query_row(
        "SELECT local_cursor, remote_cursor FROM daemon_sync_peers WHERE peer_id = ?1",
        [peer_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    let open_conflicts: i64 = database.query_row(
        "SELECT count(*) FROM daemon_sync_conflicts WHERE peer_id = ?1 AND resolved_at IS NULL",
        [peer_id],
        |row| row.get(0),
    )?;
    Ok(json!({
        "peer_id": peer_id,
        "local_cursor": local_cursor,
        "remote_cursor": remote_cursor,
        "open_conflicts": open_conflicts,
    }))
}

pub async fn acknowledge_local(daemon: &Daemon, params: Value) -> Result<Value> {
    let peer_id = peer_id(&params)?;
    let cursor = nonnegative_integer(&params, "cursor")?;
    let database = daemon.database.lock().await;
    ensure_schema(&database)?;
    ensure_peer(&database, peer_id)?;
    database.execute(
        "UPDATE daemon_sync_peers SET local_cursor = max(local_cursor, ?2), updated_at = ?3 WHERE peer_id = ?1",
        params![peer_id, cursor, Utc::now().to_rfc3339()],
    )?;
    state_value(&database, peer_id)
}

pub async fn apply_remote(daemon: &Daemon, params: Value) -> Result<Value> {
    let remote = RemoteEntity::from_params(&params)?;
    let peer_id = peer_id(&params)?;
    let remote_cursor = nonnegative_integer(&params, "remote_cursor")?;
    let mut database = daemon.database.lock().await;
    ensure_schema(&database)?;
    ensure_peer(&database, peer_id)?;
    let transaction = database.transaction()?;
    let local_cursor: i64 = transaction.query_row(
        "SELECT local_cursor FROM daemon_sync_peers WHERE peer_id = ?1",
        [peer_id],
        |row| row.get(0),
    )?;
    let current = load_entity(&transaction, &remote.entity_type, &remote.entity_id)?;
    let unresolved: Option<String> = transaction
        .query_row(
            "SELECT id FROM daemon_sync_conflicts WHERE peer_id = ?1 AND entity_type = ?2 AND entity_id = ?3 AND resolved_at IS NULL",
            params![peer_id, remote.entity_type, remote.entity_id],
            |row| row.get(0),
        )
        .optional()?;
    let pending_local: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM daemon_sync_changes WHERE cursor > ?1 AND entity_type = ?2 AND entity_id = ?3)",
        params![local_cursor, remote.entity_type, remote.entity_id],
        |row| row.get(0),
    )?;
    let same_revision_diverged = current.as_ref().is_some_and(|local| {
        local.revision == remote.revision
            && (local.tombstone != remote.tombstone
                || (!remote.tombstone && remote.payload.as_ref() != Some(&local.payload)))
    });
    if pending_local || unresolved.is_some() || same_revision_diverged {
        let conflict_id = record_conflict(
            &transaction,
            peer_id,
            unresolved.as_deref(),
            current.as_ref(),
            &remote,
        )?;
        advance_remote_cursor(&transaction, peer_id, remote_cursor)?;
        transaction.commit()?;
        return Ok(json!({
            "status": "conflict",
            "conflict_id": conflict_id,
            "entity_type": remote.entity_type,
            "entity_id": remote.entity_id,
        }));
    }
    if current
        .as_ref()
        .is_some_and(|local| local.revision > remote.revision)
    {
        advance_remote_cursor(&transaction, peer_id, remote_cursor)?;
        transaction.commit()?;
        return Ok(json!({
            "status": "ignored_stale",
            "entity_type": remote.entity_type,
            "entity_id": remote.entity_id,
            "revision": remote.revision,
        }));
    }
    write_remote_entity(&transaction, &remote)?;
    advance_remote_cursor(&transaction, peer_id, remote_cursor)?;
    transaction.commit()?;
    Ok(json!({
        "status": "applied",
        "entity_type": remote.entity_type,
        "entity_id": remote.entity_id,
        "revision": remote.revision,
    }))
}

pub async fn list_conflicts(daemon: &Daemon, params: Value) -> Result<Value> {
    let peer_id = peer_id(&params)?;
    let include_resolved = params
        .get("include_resolved")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let database = daemon.database.lock().await;
    ensure_schema(&database)?;
    let mut statement = database.prepare(
        r#"
        SELECT id, entity_type, entity_id, local_entity_json, remote_entity_json,
               created_at, resolved_at, resolution
        FROM daemon_sync_conflicts
        WHERE peer_id = ?1 AND (?2 = 1 OR resolved_at IS NULL)
        ORDER BY created_at, id
        "#,
    )?;
    let conflicts = statement
        .query_map(params![peer_id, include_resolved], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?
        .map(|row| {
            let (id, entity_type, entity_id, local, remote, created_at, resolved_at, resolution) = row?;
            Ok(json!({
                "id": id,
                "peer_id": peer_id,
                "entity_type": entity_type,
                "entity_id": entity_id,
                "local_entity": local.map(|value| serde_json::from_str::<Value>(&value)).transpose()?,
                "remote_entity": serde_json::from_str::<Value>(&remote)?,
                "created_at": created_at,
                "resolved_at": resolved_at,
                "resolution": resolution,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Value::Array(conflicts))
}

pub async fn resolve_conflict(daemon: &Daemon, params: Value) -> Result<Value> {
    let peer_id = peer_id(&params)?;
    let conflict_id = required_text(&params, "conflict_id", 100)?;
    let resolution = required_text(&params, "resolution", 32)?;
    if !matches!(resolution, "use_remote" | "keep_local") {
        bail!("resolution must be use_remote or keep_local");
    }
    let mut database = daemon.database.lock().await;
    ensure_schema(&database)?;
    let transaction = database.transaction()?;
    let row = transaction
        .query_row(
            r#"
            SELECT local_entity_json, remote_entity_json
            FROM daemon_sync_conflicts
            WHERE id = ?1 AND peer_id = ?2 AND resolved_at IS NULL
            "#,
            params![conflict_id, peer_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .context("open sync conflict was not found")?;
    let remote = RemoteEntity::from_value(serde_json::from_str(&row.1)?)?;
    match resolution {
        "use_remote" => write_remote_entity(&transaction, &remote)?,
        "keep_local" => {
            let local = row
                .0
                .map(|value| serde_json::from_str::<Value>(&value))
                .transpose()?
                .context("the conflicting local entity no longer exists")?;
            write_local_resolution(&transaction, &local, remote.revision)?;
        }
        _ => unreachable!(),
    }
    let now = Utc::now().to_rfc3339();
    transaction.execute(
        "UPDATE daemon_sync_conflicts SET resolved_at = ?3, resolution = ?4 WHERE id = ?1 AND peer_id = ?2",
        params![conflict_id, peer_id, now, resolution],
    )?;
    transaction.commit()?;
    Ok(json!({"id": conflict_id, "resolution": resolution, "resolved_at": now}))
}

fn ensure_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS daemon_sync_peers (
            peer_id TEXT PRIMARY KEY,
            local_cursor INTEGER NOT NULL DEFAULT 0 CHECK (local_cursor >= 0),
            remote_cursor INTEGER NOT NULL DEFAULT 0 CHECK (remote_cursor >= 0),
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS daemon_sync_conflicts (
            id TEXT PRIMARY KEY,
            peer_id TEXT NOT NULL REFERENCES daemon_sync_peers(peer_id) ON DELETE CASCADE,
            entity_type TEXT NOT NULL,
            entity_id TEXT NOT NULL,
            local_entity_json TEXT,
            remote_entity_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            resolved_at TEXT,
            resolution TEXT CHECK (resolution IS NULL OR resolution IN ('use_remote', 'keep_local'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS daemon_sync_conflicts_open
            ON daemon_sync_conflicts(peer_id, entity_type, entity_id)
            WHERE resolved_at IS NULL;
        "#,
    )?;
    Ok(())
}

fn ensure_peer(connection: &Connection, peer_id: &str) -> Result<()> {
    connection.execute(
        "INSERT OR IGNORE INTO daemon_sync_peers (peer_id, updated_at) VALUES (?1, ?2)",
        params![peer_id, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

fn state_value(connection: &Connection, peer_id: &str) -> Result<Value> {
    let (local_cursor, remote_cursor) = connection.query_row(
        "SELECT local_cursor, remote_cursor FROM daemon_sync_peers WHERE peer_id = ?1",
        [peer_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    Ok(json!({
        "peer_id": peer_id,
        "local_cursor": local_cursor,
        "remote_cursor": remote_cursor,
    }))
}

fn advance_remote_cursor(connection: &Connection, peer_id: &str, cursor: i64) -> Result<()> {
    connection.execute(
        "UPDATE daemon_sync_peers SET remote_cursor = max(remote_cursor, ?2), updated_at = ?3 WHERE peer_id = ?1",
        params![peer_id, cursor, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

fn record_conflict(
    connection: &Connection,
    peer_id: &str,
    existing_id: Option<&str>,
    local: Option<&LocalEntity>,
    remote: &RemoteEntity,
) -> Result<String> {
    let id = existing_id
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let local_json = local.map(serde_json::to_string).transpose()?;
    let remote_json = serde_json::to_string(&remote.as_value())?;
    if existing_id.is_some() {
        connection.execute(
            "UPDATE daemon_sync_conflicts SET local_entity_json = ?2, remote_entity_json = ?3 WHERE id = ?1",
            params![id, local_json, remote_json],
        )?;
    } else {
        connection.execute(
            r#"
            INSERT INTO daemon_sync_conflicts (
                id, peer_id, entity_type, entity_id, local_entity_json,
                remote_entity_json, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                id,
                peer_id,
                remote.entity_type,
                remote.entity_id,
                local_json,
                remote_json,
                Utc::now().to_rfc3339(),
            ],
        )?;
    }
    Ok(id)
}

fn load_entity(
    connection: &Connection,
    entity_type: &str,
    entity_id: &str,
) -> Result<Option<LocalEntity>> {
    connection
        .query_row(
            r#"
            SELECT revision, etag, payload_json, tombstone, created_at, updated_at
            FROM daemon_entities WHERE entity_type = ?1 AND id = ?2
            "#,
            params![entity_type, entity_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?
        .map(
            |(revision, etag, payload, tombstone, created_at, updated_at)| {
                Ok(LocalEntity {
                    entity_type: entity_type.to_owned(),
                    id: entity_id.to_owned(),
                    revision,
                    etag,
                    payload: serde_json::from_str(&payload)?,
                    tombstone,
                    created_at,
                    updated_at,
                })
            },
        )
        .transpose()
}

fn write_remote_entity(connection: &Connection, remote: &RemoteEntity) -> Result<()> {
    let existing = load_entity(connection, &remote.entity_type, &remote.entity_id)?;
    let payload = if remote.tombstone {
        existing
            .as_ref()
            .map(|entity| entity.payload.clone())
            .unwrap_or_else(|| json!({}))
    } else {
        remote
            .payload
            .clone()
            .context("remote upsert is missing its payload")?
    };
    let payload_json = serde_json::to_string(&payload)?;
    let created_at = existing
        .as_ref()
        .map(|entity| entity.created_at.clone())
        .unwrap_or_else(|| remote.updated_at.clone());
    let etag = format!(
        "W/\"{}:{}:{}\"",
        remote.entity_type, remote.entity_id, remote.revision
    );
    connection.execute(
        r#"
        INSERT INTO daemon_entities (
            entity_type, id, revision, etag, payload_json, tombstone, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        ON CONFLICT(entity_type, id) DO UPDATE SET
            revision = excluded.revision, etag = excluded.etag,
            payload_json = excluded.payload_json, tombstone = excluded.tombstone,
            updated_at = excluded.updated_at
        "#,
        params![
            remote.entity_type,
            remote.entity_id,
            remote.revision,
            etag,
            payload_json,
            remote.tombstone,
            created_at,
            remote.updated_at,
        ],
    )?;
    Ok(())
}

fn write_local_resolution(
    connection: &Connection,
    local: &Value,
    remote_revision: i64,
) -> Result<()> {
    let entity_type = required_text(local, "entity_type", 64)?;
    let entity_id = required_text(local, "id", 500)?;
    let payload = local
        .get("payload")
        .cloned()
        .context("local conflict payload is missing")?;
    let tombstone = local
        .get("tombstone")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let revision = remote_revision + 1;
    let now = Utc::now().to_rfc3339();
    let etag = format!("W/\"{entity_type}:{entity_id}:{revision}\"");
    let payload_json = serde_json::to_string(&payload)?;
    connection.execute(
        r#"
        INSERT INTO daemon_entities (
            entity_type, id, revision, etag, payload_json, tombstone, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
        ON CONFLICT(entity_type, id) DO UPDATE SET
            revision = excluded.revision, etag = excluded.etag,
            payload_json = excluded.payload_json, tombstone = excluded.tombstone,
            updated_at = excluded.updated_at
        "#,
        params![
            entity_type,
            entity_id,
            revision,
            etag,
            payload_json,
            tombstone,
            now,
        ],
    )?;
    let entity = load_entity(connection, entity_type, entity_id)?
        .context("resolved local entity could not be reloaded")?;
    connection.execute(
        "INSERT INTO daemon_sync_changes (entity_type, entity_id, revision, operation, entity_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            entity_type,
            entity_id,
            revision,
            if tombstone { "delete" } else { "upsert" },
            serde_json::to_string(&entity)?,
            now,
        ],
    )?;
    Ok(())
}

fn peer_id(params: &Value) -> Result<&str> {
    let value = required_text(params, "peer_id", MAX_PEER_ID_BYTES)?;
    if value.contains('\0') {
        bail!("peer_id is invalid");
    }
    Ok(value)
}

fn required_text<'a>(params: &'a Value, key: &str, maximum: usize) -> Result<&'a str> {
    let value = params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{key} is required"))?;
    if value.len() > maximum {
        bail!("{key} exceeds {maximum} bytes");
    }
    Ok(value)
}

fn nonnegative_integer(params: &Value, key: &str) -> Result<i64> {
    let value = params
        .get(key)
        .and_then(Value::as_i64)
        .with_context(|| format!("{key} must be an integer"))?;
    if value < 0 {
        bail!("{key} must not be negative");
    }
    Ok(value)
}

#[derive(Debug, serde::Serialize)]
struct LocalEntity {
    entity_type: String,
    id: String,
    revision: i64,
    etag: String,
    payload: Value,
    tombstone: bool,
    created_at: String,
    updated_at: String,
}

#[derive(Debug)]
struct RemoteEntity {
    entity_type: String,
    entity_id: String,
    revision: i64,
    payload: Option<Value>,
    tombstone: bool,
    updated_at: String,
}

impl RemoteEntity {
    fn from_params(params: &Value) -> Result<Self> {
        Self::from_value(
            params
                .get("entity")
                .cloned()
                .context("entity is required")?,
        )
    }

    fn from_value(value: Value) -> Result<Self> {
        let entity_type = required_text(&value, "entity_type", 64)?.to_owned();
        let entity_id = value
            .get("entity_id")
            .or_else(|| value.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("entity_id is required")?
            .to_owned();
        let revision = value
            .get("revision")
            .and_then(Value::as_i64)
            .context("entity revision must be an integer")?;
        if revision < 1 {
            bail!("entity revision must be positive");
        }
        let tombstone = value
            .get("tombstone")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| value.get("operation").and_then(Value::as_str) == Some("delete"));
        let payload = value.get("payload").cloned().filter(|item| !item.is_null());
        if !tombstone && !payload.as_ref().is_some_and(Value::is_object) {
            bail!("remote upsert payload must be an object");
        }
        if payload.as_ref().is_some_and(|payload| {
            serde_json::to_vec(payload).is_ok_and(|encoded| encoded.len() > MAX_SYNC_PAYLOAD_BYTES)
        }) {
            bail!("remote payload exceeds {MAX_SYNC_PAYLOAD_BYTES} bytes");
        }
        let updated_at = value
            .get("updated_at")
            .or_else(|| value.get("created_at"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let updated_at = if updated_at.is_empty() {
            Utc::now().to_rfc3339()
        } else {
            updated_at
        };
        Ok(Self {
            entity_type,
            entity_id,
            revision,
            payload,
            tombstone,
            updated_at,
        })
    }

    fn as_value(&self) -> Value {
        json!({
            "entity_type": self.entity_type,
            "entity_id": self.entity_id,
            "revision": self.revision,
            "payload": self.payload,
            "tombstone": self.tombstone,
            "updated_at": self.updated_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_entities_enforce_payload_boundaries() {
        assert!(RemoteEntity::from_value(json!({
            "entity_type": "memory",
            "entity_id": Uuid::new_v4(),
            "revision": 1,
            "payload": {"text": "safe"},
            "tombstone": false,
            "updated_at": Utc::now(),
        }))
        .is_ok());
        assert!(RemoteEntity::from_value(json!({
            "entity_type": "memory",
            "entity_id": Uuid::new_v4(),
            "revision": 1,
            "payload": null,
            "tombstone": false,
        }))
        .is_err());
    }
}
