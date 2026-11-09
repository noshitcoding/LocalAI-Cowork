//! Construct an actual historical schema in a fresh owned fixture database.
//! Never remove migration receipts from a current schema and pretend it is old.
use sqlx::Row;
use std::path::Path;

const SCHEMA: &[&str] = &[
    include_str!("../../migrations/0001.sql"),
    include_str!("../../migrations/0002.sql"),
    include_str!("../../migrations/0003.sql"),
    include_str!("../../migrations/0004.sql"),
    include_str!("../../migrations/0005.sql"),
    include_str!("../../migrations/0006.sql"),
    include_str!("../../migrations/0007.sql"),
    include_str!("../../migrations/0008.sql"),
    include_str!("../../migrations/0009.sql"),
    include_str!("../../migrations/0010.sql"),
    include_str!("../../migrations/0011.sql"),
    include_str!("../../migrations/0012.sql"),
    include_str!("../../migrations/0013.sql"),
    include_str!("../../migrations/0014.sql"),
    include_str!("../../migrations/0015.sql"),
    include_str!("../../migrations/0016.sql"),
    include_str!("../../migrations/0017.sql"),
];
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
pub(super) async fn copy(source: &Path, target: &Path, through: usize) -> i64 {
    assert!(source.is_file() && !target.exists());
    assert_eq!(
        source.parent(),
        target.parent(),
        "Use one owned fixture directory"
    );
    assert!((1..=SCHEMA.len()).contains(&through));
    let mut connection = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(target)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE schema_migrations(version BIGINT PRIMARY KEY,checksum TEXT NOT NULL)",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    for sql in SCHEMA.iter().take(through) {
        for statement in sql
            .split(';')
            .map(str::trim)
            .filter(|statement| !statement.is_empty())
        {
            sqlx::query(statement)
                .execute(&mut connection)
                .await
                .unwrap();
        }
    }
    sqlx::query("ATTACH DATABASE ? AS prior")
        .bind(source.to_str().unwrap())
        .execute(&mut connection)
        .await
        .unwrap();
    let current_version: i64 =
        sqlx::query_scalar("SELECT MAX(version) FROM prior.schema_migrations")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    let tables:Vec<String>=sqlx::query_scalar("SELECT name FROM main.sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name<>'schema_migrations'").fetch_all(&mut connection).await.unwrap();
    for table in tables {
        // Schema guard rows belong to the historical DDL, not to the copied
        // workflow data. Later migrations advance that original guard.
        if table.ends_with("_schema_guard") {
            continue;
        }
        let exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM prior.sqlite_master WHERE type='table' AND name=?",
        )
        .bind(&table)
        .fetch_one(&mut connection)
        .await
        .unwrap();
        if exists == 0 {
            continue;
        }
        let old = sqlx::query(&format!("PRAGMA main.table_info({})", quoted(&table)))
            .fetch_all(&mut connection)
            .await
            .unwrap();
        let current = sqlx::query(&format!("PRAGMA prior.table_info({})", quoted(&table)))
            .fetch_all(&mut connection)
            .await
            .unwrap();
        let available: std::collections::BTreeSet<String> =
            current.iter().map(|row| row.get("name")).collect();
        let columns = old
            .iter()
            .map(|row| row.get::<String, _>("name"))
            .filter(|column| available.contains(column))
            .map(|column| quoted(&column))
            .collect::<Vec<_>>()
            .join(",");
        sqlx::query(&format!(
            "INSERT INTO main.{}({columns}) SELECT {columns} FROM prior.{}",
            quoted(&table),
            quoted(&table)
        ))
        .execute(&mut connection)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO main.schema_migrations SELECT version,checksum FROM prior.schema_migrations WHERE version<=?").bind(through as i64).execute(&mut connection).await.unwrap();
    sqlx::query("DETACH DATABASE prior")
        .execute(&mut connection)
        .await
        .unwrap();
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut connection)
            .await
            .unwrap()
            .is_empty()
    );
    use sqlx::Connection;
    connection.close().await.unwrap();
    current_version
}
