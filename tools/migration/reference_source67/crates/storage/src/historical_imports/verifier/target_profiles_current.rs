//! Explicit separately pinned current cold-target artifacts. Never live authority.
use super::{Error, Result, canonical as c, profiles, quote};
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection};

pub(super) const PREVIEW_FORMAT: &str = "localai-cowork-quarantine-preview/2";
const FORMAT: &str = "localai-cowork-reviewed-cold-target-profile/1";
const ROLE: &str = "cold provenance only; no current authority or execution adoption";

/// Constructed only from a root-owned embedded artifact and its compiled pin.
/// Callers must never pass a renderer/HTTP supplied expected hash.
pub(super) struct CurrentTargetProfile {
    pub(super) value: Value,
    manifest_sha256: String,
}
impl CurrentTargetProfile {
    pub(super) fn profile(&self) -> &str {
        self.value["profile"].as_str().unwrap_or("")
    }
    pub(super) fn descriptor(&self) -> Value {
        json!({"profile":self.value["profile"],"schema_sha256":self.value["schema_sha256"],
            "source_bindings":self.value["source_bindings"],"profile_manifest_sha256":self.manifest_sha256,
            "source_closure_sha256":self.value["source_closure_sha256"],
            "producer_source_pins":self.value["producer_source_pins"]})
    }
}

pub(super) fn verified(bytes: &[u8], fixed_sha: &str) -> Result<CurrentTargetProfile> {
    c::sha(&json!(fixed_sha))?;
    if c::hex(&c::hash(bytes)) != fixed_sha {
        return Err(Error::Refused(
            "Current fixed target profile artifact changed",
        ));
    }
    let value = c::parse(bytes, 2 * 1024 * 1024)?;
    c::object(
        &value,
        &[
            "format",
            "profile",
            "central_version",
            "schema",
            "schema_sha256",
            "migrations",
            "source_bindings",
            "source_closure_sha256",
            "producer_source_pins",
            "guards",
            "integer_ids",
            "ddl",
            "role",
        ],
    )?;
    let expected = match c::string(&value["profile"])? {
        "v2-storage-64-complete-store-connect/1" => 64,
        "v2-storage-66-complete-store-connect/1" => 66,
        "v2-storage-67-complete-store-connect/1" => 67,
        _ => {
            return Err(Error::Refused(
                "Unknown explicitly registered current target profile",
            ));
        }
    };
    if value["format"] != FORMAT
        || value["central_version"] != expected
        || value["role"] != ROLE
        || profiles::signature(&value["schema"])? != c::sha(&value["schema_sha256"])?
    {
        return Err(Error::Refused("Current target profile descriptor differs"));
    }
    c::sha(&value["source_closure_sha256"])?;
    let migrations = value["migrations"]
        .as_array()
        .ok_or(Error::Refused("Current exact migrations"))?;
    if migrations.len() != expected as usize {
        return Err(Error::Refused(
            "Current target migration registry incomplete",
        ));
    }
    for (index, migration) in migrations.iter().enumerate() {
        let row = migration
            .as_array()
            .ok_or(Error::Refused("Current exact migration entry"))?;
        if row.len() != 2 || row[0] != json!(index + 1) {
            return Err(Error::Refused("Current target migration sequence differs"));
        }
        c::sha(&row[1])?;
    }
    for field in ["source_bindings", "producer_source_pins"] {
        let sources = value[field]
            .as_object()
            .ok_or(Error::Refused("Current profile source bindings"))?;
        if sources.is_empty() || sources.len() > 512 {
            return Err(Error::Refused("Current profile source closure count"));
        }
        for (path, pin) in sources {
            if path.is_empty()
                || path.starts_with('/')
                || path.contains('\\')
                || path.contains(':')
                || path
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
            {
                return Err(Error::Refused("Current fixed source binding path"));
            }
            c::sha(pin)?;
        }
    }
    if !value["guards"].is_object() || !value["integer_ids"].is_object() || !value["ddl"].is_array()
    {
        return Err(Error::Refused("Current profile fixed guard rules"));
    }
    Ok(CurrentTargetProfile {
        value,
        manifest_sha256: fixed_sha.to_owned(),
    })
}

pub(super) async fn check_schema(
    conn: &mut SqliteConnection,
    profile: &CurrentTargetProfile,
) -> Result<()> {
    // SQL is recognized against the complete reviewed object set before PRAGMAs
    // or integrity work. No input-provided SQL is executed.
    profiles::schema(conn, &profile.value["schema"]).await?;
    let rows = sqlx::query("SELECT version,checksum FROM schema_migrations ORDER BY version")
        .fetch_all(&mut *conn)
        .await?;
    let entries: Vec<Value> = rows
        .iter()
        .map(|row| {
            Ok(json!([
                row.try_get::<i64, _>(0)?,
                row.try_get::<String, _>(1)?
            ]))
        })
        .collect::<Result<_>>()?;
    if json!(entries) != profile.value["migrations"] {
        return Err(Error::Refused(
            "Current target exact migration checksums differ",
        ));
    }
    for (table, version) in profile.value["guards"]
        .as_object()
        .ok_or(Error::Refused("Current guards"))?
    {
        let rows = sqlx::query(&format!(
            "SELECT version,typeof(version) FROM {} LIMIT 2",
            quote(table)
        ))
        .fetch_all(&mut *conn)
        .await?;
        if rows.len() != 1
            || json!(rows[0].try_get::<i64, _>(0)?) != *version
            || rows[0].try_get::<String, _>(1)? != "integer"
        {
            return Err(Error::Refused(
                "Current target guarded schema version differs",
            ));
        }
    }
    let tables = profile.value["schema"]["tables"]
        .as_object()
        .ok_or(Error::Refused("Current target tables"))?;
    let rules = profile.value["integer_ids"]
        .as_object()
        .ok_or(Error::Refused("Current target integer rules"))?;
    let mut integer_tables = std::collections::BTreeSet::new();
    for (table, metadata) in tables {
        let columns = metadata["columns"]
            .as_array()
            .ok_or(Error::Refused("Current target columns"))?;
        if columns.iter().any(|column| {
            column[1] == "id" && matches!(column[2].as_str(), Some("BIGINT" | "INTEGER"))
        }) {
            integer_tables.insert(table.as_str());
        }
    }
    if integer_tables != rules.keys().map(String::as_str).collect() {
        return Err(Error::Refused("Unreviewed current integer identity table"));
    }
    for (table, rule) in rules {
        c::object(rule, &["value", "required"])?;
        let expected = rule["value"]
            .as_i64()
            .ok_or(Error::Refused("Current integer identity value"))?;
        let required = rule["required"]
            .as_bool()
            .ok_or(Error::Refused("Current integer identity presence"))?;
        let rows = sqlx::query(&format!(
            "SELECT id,typeof(id) FROM {} LIMIT 2",
            quote(table)
        ))
        .fetch_all(&mut *conn)
        .await?;
        if rows.is_empty() && !required {
            continue;
        }
        if rows.len() != 1
            || rows[0].try_get::<i64, _>(0)? != expected
            || rows[0].try_get::<String, _>(1)? != "integer"
        {
            return Err(Error::Refused(
                "Current target exact singleton identity differs",
            ));
        }
    }
    for name in ["user_version", "application_id"] {
        let value: i64 = sqlx::query_scalar(&format!("PRAGMA history_data.{name}"))
            .fetch_one(&mut *conn)
            .await?;
        if value != 0 {
            return Err(Error::Refused("Current target SQLite metadata version"));
        }
    }
    profiles::integrity(conn).await
}
