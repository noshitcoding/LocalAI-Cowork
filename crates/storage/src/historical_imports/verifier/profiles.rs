use super::{
    Error, Result,
    canonical::{self as c, *},
    quote,
};
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection};

pub(super) const PROFILE_JSON: &[u8] = include_bytes!("data/profiles.json");
pub(super) const PROFILE_SHA: &str =
    "346fb4292436b1906bacb8d2a91719b12cc85a8331281039de63d75fc859c31f";

pub(super) fn fixed() -> Result<Value> {
    if hex(&hash(PROFILE_JSON)) != PROFILE_SHA {
        return Err(Error::Refused("Fixed complete profile data changed"));
    }
    let value = parse(PROFILE_JSON, 2 * 1024 * 1024)?;
    object(
        &value,
        &[
            "format",
            "producer_source_pins",
            "source_bindings",
            "sources",
            "quarantine",
            "historical_target",
            "role",
        ],
    )?;
    if value["format"] != "localai-cowork-fixed-historical-verifier-profiles/1" {
        return Err(Error::Refused("Fixed profile format"));
    }
    Ok(value)
}

pub(super) fn source<'a>(
    fixed: &'a Value,
    family: &str,
    version: &Value,
    name: &str,
) -> Result<&'a Value> {
    fixed["sources"]
        .as_array()
        .ok_or(Error::Refused("Fixed sources"))?
        .iter()
        .find(|p| p["family"] == family && p["version"] == *version && p["profile"] == name)
        .ok_or(Error::Refused("Unknown full V1 schema/version/profile"))
}

fn tokens(sql: &str) -> Result<Vec<String>> {
    let chars: Vec<_> = sql.chars().collect();
    let mut at = 0;
    let mut result = Vec::new();
    while at < chars.len() {
        let start = at;
        let ch = chars[at];
        if ch.is_whitespace() {
            at += 1;
            continue;
        }
        if matches!(ch, '\'' | '"' | '[' | '`') {
            let close = if ch == '[' { ']' } else { ch };
            at += 1;
            loop {
                if at >= chars.len() {
                    return Err(Error::Refused("Unterminated schema SQL token"));
                }
                if chars[at] == close {
                    at += 1;
                    if matches!(ch, '\'' | '"') && at < chars.len() && chars[at] == close {
                        at += 1;
                        continue;
                    }
                    break;
                }
                at += 1;
            }
            result.push(chars[start..at].iter().collect());
        } else if ch.is_ascii_alphabetic() || ch == '_' {
            at += 1;
            while at < chars.len() && (chars[at].is_ascii_alphanumeric() || chars[at] == '_') {
                at += 1;
            }
            result.push(
                chars[start..at]
                    .iter()
                    .collect::<String>()
                    .to_ascii_lowercase(),
            );
        } else if ch.is_ascii_digit() {
            at += 1;
            while at < chars.len() && chars[at].is_ascii_digit() {
                at += 1;
            }
            if at + 1 < chars.len() && chars[at] == '.' && chars[at + 1].is_ascii_digit() {
                at += 2;
                while at < chars.len() && chars[at].is_ascii_digit() {
                    at += 1;
                }
            }
            result.push(chars[start..at].iter().collect());
        } else {
            at += 1;
            result.push(ch.to_lowercase().collect());
        }
    }
    Ok(result)
}

pub(super) fn normalized_objects(schema: &Value) -> Result<Value> {
    let mut objects = schema["objects"]
        .as_array()
        .ok_or(Error::Refused("Schema objects"))?
        .clone();
    for object in &mut objects {
        c::object(object, &["type", "name", "table", "sql"])?;
        object["sql"] = if object["sql"].is_null() {
            Value::Null
        } else {
            json!(tokens(string(&object["sql"])?)?)
        };
    }
    Ok(Value::Array(objects))
}

pub(super) fn signature(schema: &Value) -> Result<String> {
    object(schema, &["objects", "tables"])?;
    hash_value(&json!({"objects":normalized_objects(schema)?,"tables":schema["tables"]}))
}

pub(super) async fn schema(conn: &mut SqliteConnection, expected: &Value) -> Result<Value> {
    let expected_objects = normalized_objects(expected)?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM history_data.sqlite_schema")
        .fetch_one(&mut *conn)
        .await?;
    if count
        != expected["objects"]
            .as_array()
            .ok_or(Error::Refused("Fixed schema objects"))?
            .len() as i64
    {
        return Err(Error::Refused(
            "Unknown or additional database schema objects",
        ));
    }
    let rows = sqlx::query("SELECT type,name,tbl_name,sql FROM history_data.sqlite_schema WHERE sql IS NULL OR length(CAST(sql AS BLOB))<=131072 ORDER BY type,name")
        .fetch_all(&mut *conn).await?;
    if rows.len() as i64 != count {
        return Err(Error::Refused("Schema SQL byte limit"));
    }
    let mut objects = Vec::new();
    for row in rows {
        objects.push(json!({"type":row.try_get::<String,_>(0)?,"name":row.try_get::<String,_>(1)?,
                           "table":row.try_get::<String,_>(2)?,"sql":row.try_get::<Option<String>,_>(3)?}));
    }
    let shell = json!({"objects":objects,"tables":{}});
    // Refuse unknown virtual tables/triggers/functions before table PRAGMAs or integrity work.
    if normalized_objects(&shell)? != expected_objects {
        return Err(Error::Refused("Unknown/changed schema SQL"));
    }
    let mut tables = serde_json::Map::new();
    for object in shell["objects"]
        .as_array()
        .ok_or(Error::Refused("Schema objects"))?
    {
        if object["type"] != "table" {
            continue;
        }
        let name = string(&object["name"])?;
        let mut columns = Vec::new();
        let mut foreign = Vec::new();
        let mut indexes = Vec::new();
        for row in sqlx::query(&format!("PRAGMA history_data.table_xinfo({})", quote(name)))
            .fetch_all(&mut *conn)
            .await?
        {
            columns.push(json!([
                row.try_get::<i64, _>(0)?,
                row.try_get::<String, _>(1)?,
                row.try_get::<String, _>(2)?,
                row.try_get::<i64, _>(3)?,
                row.try_get::<Option<String>, _>(4)?,
                row.try_get::<i64, _>(5)?,
                row.try_get::<i64, _>(6)?
            ]));
        }
        for row in sqlx::query(&format!(
            "PRAGMA history_data.foreign_key_list({})",
            quote(name)
        ))
        .fetch_all(&mut *conn)
        .await?
        {
            foreign.push(json!([
                row.try_get::<i64, _>(0)?,
                row.try_get::<i64, _>(1)?,
                row.try_get::<String, _>(2)?,
                row.try_get::<String, _>(3)?,
                row.try_get::<Option<String>, _>(4)?,
                row.try_get::<String, _>(5)?,
                row.try_get::<String, _>(6)?,
                row.try_get::<String, _>(7)?
            ]));
        }
        for row in sqlx::query(&format!("PRAGMA history_data.index_list({})", quote(name)))
            .fetch_all(&mut *conn)
            .await?
        {
            indexes.push(json!([
                row.try_get::<i64, _>(0)?,
                row.try_get::<String, _>(1)?,
                row.try_get::<i64, _>(2)?,
                row.try_get::<String, _>(3)?,
                row.try_get::<i64, _>(4)?
            ]));
        }
        tables.insert(
            name.to_owned(),
            json!({"columns":columns,"foreign_keys":foreign,"indexes":indexes}),
        );
    }
    let actual = json!({"objects":shell["objects"],"tables":tables});
    if signature(&actual)? != signature(expected)? {
        return Err(Error::Refused(
            "Complete database schema fingerprint differs",
        ));
    }
    Ok(actual)
}

pub(super) async fn integrity(conn: &mut SqliteConnection) -> Result<()> {
    let rows = sqlx::query("PRAGMA history_data.integrity_check(1)")
        .fetch_all(&mut *conn)
        .await?;
    if rows.len() != 1 || rows[0].try_get::<String, _>(0)? != "ok" {
        return Err(Error::Refused("Private SQLite integrity check failed"));
    }
    if sqlx::query("PRAGMA history_data.foreign_key_check")
        .fetch_optional(&mut *conn)
        .await?
        .is_some()
    {
        return Err(Error::Refused("Historical foreign key check failed"));
    }
    Ok(())
}

pub(super) async fn historical_target(conn: &mut SqliteConnection, fixed: &Value) -> Result<()> {
    let profile = &fixed["historical_target"];
    schema(conn, &profile["schema"]).await?;
    let rows = sqlx::query("SELECT version,checksum FROM schema_migrations ORDER BY version")
        .fetch_all(&mut *conn)
        .await?;
    let values: Vec<_> = rows
        .iter()
        .map(|r| Ok(json!([r.try_get::<i64, _>(0)?, r.try_get::<String, _>(1)?])))
        .collect::<Result<_>>()?;
    if json!(values) != profile["migrations"] {
        return Err(Error::Refused(
            "Historical target migration checksums differ",
        ));
    }
    for name in [
        "identity_schema_guard",
        "tenant_project_schema_guard",
        "native_authority_schema_guard",
        "model_accounting_schema_guard",
    ] {
        let rows = sqlx::query(&format!(
            "SELECT version,typeof(version) FROM {}",
            quote(name)
        ))
        .fetch_all(&mut *conn)
        .await?;
        if rows.len() != 1
            || rows[0].try_get::<i64, _>(0)? != 1
            || rows[0].try_get::<String, _>(1)? != "integer"
        {
            return Err(Error::Refused("Unknown target guarded schema version"));
        }
    }
    for name in ["user_version", "application_id"] {
        let value: i64 = sqlx::query_scalar(&format!("PRAGMA history_data.{name}"))
            .fetch_one(&mut *conn)
            .await?;
        if value != 0 {
            return Err(Error::Refused("Historical target SQLite metadata version"));
        }
    }
    integrity(conn).await
}
