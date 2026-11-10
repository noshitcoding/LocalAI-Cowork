use super::{Budget, Error, MAX_LINE, MAX_ROWS, Result, canonical as c, quote, uuid};
use futures_util::TryStreamExt;
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};

pub(super) const ENTITY_TABLES: [(&str, &str); 8] = [
    ("projects", "project"),
    ("chat_threads", "thread"),
    ("engine_runs", "engine_run"),
    ("crew_runs", "crew_run"),
    ("artifact_versions", "artifact_version"),
    ("engine_run_artifacts", "engine_artifact"),
    ("agent_personalities", "agent_persona"),
    ("daemon_runs", "daemon_run"),
];
pub(super) const PROJECTIONS: [(&str, &[&str]); 4] = [
    (
        "history_projects",
        &["source_id", "source_table", "source_rowid"],
    ),
    ("history_actors", &["source_id"]),
    (
        "history_entities",
        &[
            "kind",
            "source_id",
            "source_table",
            "source_rowid",
            "project_source_id",
            "actor_source_id",
        ],
    ),
    (
        "history_relations",
        &[
            "kind",
            "source_kind",
            "source_id",
            "target_kind",
            "target_id",
        ],
    ),
];
pub(super) struct ProjectionProof {
    pub name: &'static str,
    pub hashes: std::sync::Arc<[[u8; 32]]>,
}

fn identifier(raw: &str) -> Result<&str> {
    c::identifier(raw)
}
async fn insert_entity(
    target: &mut SqliteConnection,
    kind: &str,
    id: &str,
    table: Option<&str>,
    rowid: Option<i64>,
    project: Option<&str>,
    actor: Option<&str>,
) -> Result<()> {
    sqlx::query("INSERT INTO history_entities VALUES(?,?,?,?,?,?)")
        .bind(kind)
        .bind(id)
        .bind(table)
        .bind(rowid)
        .bind(project)
        .bind(actor)
        .execute(target)
        .await?;
    Ok(())
}

pub(super) async fn derive(
    source: &mut SqliteConnection,
    target: &mut SqliteConnection,
    selected: &Value,
    budget: Budget,
) -> Result<()> {
    let tables = selected["schema"]["tables"]
        .as_object()
        .ok_or(Error::Refused("Fixed source tables"))?;
    for (name, metadata) in tables {
        let columns = metadata["columns"]
            .as_array()
            .ok_or(Error::Refused("Fixed source columns"))?;
        if !columns.iter().any(|c| c[1] == "id") {
            continue;
        }
        let query = format!(
            "SELECT canonical_id FROM (SELECT CASE WHEN typeof(id) IN ('text','integer') THEN CAST(id AS TEXT) ELSE NULL END AS canonical_id FROM {}) GROUP BY canonical_id HAVING canonical_id IS NULL OR count(*)>1 LIMIT 1",
            quote(name)
        );
        if sqlx::query(&query)
            .fetch_optional(&mut *source)
            .await?
            .is_some()
        {
            return Err(Error::Refused(
                "Duplicate/null/ambiguously typed original ID",
            ));
        }
        let query = format!("SELECT typeof(id),CAST(id AS BLOB) FROM {}", quote(name));
        let mut cursor = sqlx::query(&query).fetch(&mut *source);
        while let Some(row) = cursor.try_next().await? {
            budget.check()?;
            let raw: Vec<u8> = row.try_get(1)?;
            let id = std::str::from_utf8(&raw)
                .map_err(|_| Error::Refused("Historical identity is not UTF8"))?;
            identifier(id)?;
        }
    }
    if tables.contains_key("projects") {
        let mut cursor = sqlx::query("SELECT _rowid_,id FROM projects").fetch(&mut *source);
        while let Some(row) = cursor.try_next().await? {
            budget.check()?;
            let id: String = row.try_get(1)?;
            identifier(&id)?;
            sqlx::query("INSERT INTO history_projects VALUES(?,?,?)")
                .bind(id)
                .bind("projects")
                .bind(row.try_get::<i64, _>(0)?)
                .execute(&mut *target)
                .await?;
        }
    }
    if selected["family"] == "daemon" {
        let mut cursor =
            sqlx::query("SELECT _rowid_,id,record_json FROM daemon_runs").fetch(&mut *source);
        while let Some(row) = cursor.try_next().await? {
            budget.check()?;
            let rowid: i64 = row.try_get(0)?;
            let id: String = row.try_get(1)?;
            identifier(&id)?;
            let encoded: String = row.try_get(2)?;
            let record = c::parse(encoded.as_bytes(), MAX_LINE)?;
            let spec = record
                .get("spec")
                .and_then(Value::as_object)
                .ok_or(Error::Refused("Daemon historical spec missing"))?;
            if record.get("id").and_then(Value::as_str) != Some(&id) {
                return Err(Error::Refused("Daemon historical RunRecord ID differs"));
            }
            let project = uuid(
                spec.get("project_id")
                    .ok_or(Error::Refused("Original daemon project ID missing"))?,
            )?
            .to_string();
            let actor = uuid(
                spec.get("creator_user_id")
                    .ok_or(Error::Refused("Original daemon actor ID missing"))?,
            )?
            .to_string();
            if let Some(value) = spec.get("project")
                && (!value.is_object() || value.get("id").and_then(Value::as_str) != Some(&project))
            {
                return Err(Error::Refused("Original daemon project object conflicts"));
            }
            sqlx::query("INSERT OR IGNORE INTO history_projects VALUES(?,NULL,NULL)")
                .bind(&project)
                .execute(&mut *target)
                .await?;
            sqlx::query("INSERT OR IGNORE INTO history_actors VALUES(?)")
                .bind(&actor)
                .execute(&mut *target)
                .await?;
            sqlx::query("INSERT OR IGNORE INTO history_entities VALUES('historical_actor',?,NULL,NULL,NULL,NULL)").bind(&actor).execute(&mut *target).await?;
            insert_entity(
                target,
                "daemon_run",
                &id,
                Some("daemon_runs"),
                Some(rowid),
                Some(&project),
                Some(&actor),
            )
            .await?;
        }
        drop(cursor);
        if tables.contains_key("daemon_project_bindings") {
            let mut cursor =
                sqlx::query("SELECT project_id FROM daemon_project_bindings").fetch(&mut *source);
            while let Some(row) = cursor.try_next().await? {
                budget.check()?;
                let id = uuid(&json!(row.try_get::<String, _>(0)?))?.to_string();
                sqlx::query("INSERT OR IGNORE INTO history_projects VALUES(?,NULL,NULL)")
                    .bind(id)
                    .execute(&mut *target)
                    .await?;
            }
        }
        sqlx::query("INSERT INTO history_entities SELECT 'project',source_id,NULL,NULL,source_id,NULL FROM history_projects").execute(&mut *target).await?;
        return Ok(());
    }
    if tables.contains_key("project_threads") {
        if sqlx::query("SELECT thread_id FROM project_threads GROUP BY thread_id HAVING count(DISTINCT project_id)>1 LIMIT 1").fetch_optional(&mut *source).await?.is_some(){return Err(Error::Refused("Original thread has multiple project scopes"));}
        let mut cursor =
            sqlx::query("SELECT project_id,thread_id FROM project_threads").fetch(&mut *source);
        while let Some(row) = cursor.try_next().await? {
            budget.check()?;
            identifier(&row.try_get::<String, _>(0)?)?;
            identifier(&row.try_get::<String, _>(1)?)?;
        }
    }
    for (table, kind) in ENTITY_TABLES {
        let Some(metadata) = tables.get(table) else {
            continue;
        };
        if kind == "daemon_run" {
            continue;
        }
        let has_thread = metadata["columns"]
            .as_array()
            .ok_or(Error::Refused("Source columns"))?
            .iter()
            .any(|c| c[1] == "thread_id");
        let project = if kind == "project" {
            "t.id".to_owned()
        } else if tables.contains_key("project_threads") && (kind == "thread" || has_thread) {
            format!(
                "(SELECT p.project_id FROM project_threads p WHERE p.thread_id=t.{})",
                if kind == "thread" { "id" } else { "thread_id" }
            )
        } else {
            "NULL".to_owned()
        };
        let query = format!("SELECT t._rowid_,t.id,{project} FROM {} t", quote(table));
        let mut cursor = sqlx::query(&query).fetch(&mut *source);
        while let Some(row) = cursor.try_next().await? {
            budget.check()?;
            let id: String = row.try_get(1)?;
            identifier(&id)?;
            let project: Option<String> = row.try_get(2)?;
            insert_entity(
                target,
                kind,
                &id,
                Some(table),
                Some(row.try_get(0)?),
                project.as_deref(),
                None,
            )
            .await?;
        }
    }
    if tables.contains_key("project_threads") {
        let mut cursor =
            sqlx::query("SELECT project_id,thread_id FROM project_threads").fetch(&mut *source);
        while let Some(row) = cursor.try_next().await? {
            budget.check()?;
            let project: String = row.try_get(0)?;
            let thread: String = row.try_get(1)?;
            if sqlx::query("SELECT 1 FROM history_entities WHERE kind='thread' AND source_id=?")
                .bind(&thread)
                .fetch_optional(&mut *target)
                .await?
                .is_none()
            {
                return Err(Error::Refused(
                    "Project binding references absent original thread",
                ));
            }
            sqlx::query(
                "INSERT INTO history_relations VALUES('project_thread','project',?,'thread',?)",
            )
            .bind(project)
            .bind(thread)
            .execute(&mut *target)
            .await?;
        }
    }
    if tables.contains_key("engine_runs") {
        let mut cursor =
            sqlx::query("SELECT id,parent_run_id FROM engine_runs WHERE parent_run_id IS NOT NULL")
                .fetch(&mut *source);
        while let Some(row) = cursor.try_next().await? {
            budget.check()?;
            let id: String = row.try_get(0)?;
            let parent: String = row.try_get(1)?;
            identifier(&parent)?;
            let a=sqlx::query("SELECT project_source_id FROM history_entities WHERE kind='engine_run' AND source_id=?").bind(&id).fetch_one(&mut *target).await?;
            let b=sqlx::query("SELECT project_source_id FROM history_entities WHERE kind='engine_run' AND source_id=?").bind(&parent).fetch_optional(&mut *target).await?.ok_or(Error::Refused("Original parent run missing"))?;
            if a.try_get::<Option<String>, _>(0)? != b.try_get::<Option<String>, _>(0)? {
                return Err(Error::Refused("Original parent run crosses project"));
            }
            sqlx::query(
                "INSERT INTO history_relations VALUES('parent_run','engine_run',?,'engine_run',?)",
            )
            .bind(id)
            .bind(parent)
            .execute(&mut *target)
            .await?;
        }
    }
    for (table, kind) in [
        ("artifact_versions", "artifact_version"),
        ("engine_run_artifacts", "engine_artifact"),
    ] {
        if !tables.contains_key(table) {
            continue;
        }
        let query = format!("SELECT id,run_id FROM {}", quote(table));
        let mut cursor = sqlx::query(&query).fetch(&mut *source);
        while let Some(row) = cursor.try_next().await? {
            budget.check()?;
            let id: String = row.try_get(0)?;
            let run: String = row.try_get(1)?;
            let candidates=sqlx::query("SELECT kind,project_source_id FROM history_entities WHERE kind IN ('engine_run','crew_run','daemon_run') AND source_id=?").bind(&run).fetch_all(&mut *target).await?;
            if candidates.len() != 1 {
                return Err(Error::Refused(
                    "Artifact original run binding absent/ambiguous",
                ));
            }
            let run_kind: String = candidates[0].try_get(0)?;
            let project: Option<String> = candidates[0].try_get(1)?;
            sqlx::query(
                "UPDATE history_entities SET project_source_id=? WHERE kind=? AND source_id=?",
            )
            .bind(project)
            .bind(kind)
            .bind(&id)
            .execute(&mut *target)
            .await?;
            sqlx::query("INSERT INTO history_relations VALUES('run_artifact',?,?,?,?)")
                .bind(run_kind)
                .bind(run)
                .bind(kind)
                .bind(id)
                .execute(&mut *target)
                .await?;
        }
    }
    Ok(())
}

pub(super) fn projection_value(name: &str, row: &SqliteRow) -> Result<Value> {
    Ok(match name {
        "history_projects" => json!([
            row.try_get::<String, _>(0)?,
            row.try_get::<Option<String>, _>(1)?,
            row.try_get::<Option<i64>, _>(2)?
        ]),
        "history_actors" => json!([row.try_get::<String, _>(0)?]),
        "history_entities" => json!([
            row.try_get::<String, _>(0)?,
            row.try_get::<String, _>(1)?,
            row.try_get::<Option<String>, _>(2)?,
            row.try_get::<Option<i64>, _>(3)?,
            row.try_get::<Option<String>, _>(4)?,
            row.try_get::<Option<String>, _>(5)?
        ]),
        "history_relations" => json!([
            row.try_get::<String, _>(0)?,
            row.try_get::<String, _>(1)?,
            row.try_get::<String, _>(2)?,
            row.try_get::<String, _>(3)?,
            row.try_get::<String, _>(4)?
        ]),
        _ => return Err(Error::Refused("Unknown derived projection")),
    })
}

pub(super) async fn compare(
    original: &mut SqliteConnection,
    expected: &mut SqliteConnection,
    budget: Budget,
) -> Result<Vec<ProjectionProof>> {
    let mut proofs = Vec::new();
    let mut total = 0;
    for (name, columns) in PROJECTIONS {
        let query = format!(
            "SELECT * FROM {} ORDER BY {}",
            quote(name),
            columns
                .iter()
                .map(|c| quote(c))
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut actual = sqlx::query(&query).fetch(&mut *original);
        let mut derived = sqlx::query(&query).fetch(&mut *expected);
        let mut hashes = Vec::new();
        loop {
            budget.check()?;
            let a = actual.try_next().await?;
            let b = derived.try_next().await?;
            match (a, b) {
                (None, None) => break,
                (Some(a), Some(b)) => {
                    total += 1;
                    if total > MAX_ROWS * 6 {
                        return Err(Error::Refused("Derived projection limit"));
                    }
                    let a = projection_value(name, &a)?;
                    let b = projection_value(name, &b)?;
                    if a != b {
                        return Err(Error::Refused(
                            "Historical ID/project/actor/relation projection differs from original rows",
                        ));
                    }
                    hashes.push(c::hash(&c::canonical(&a)?));
                }
                _ => return Err(Error::Refused("Historical projection count differs")),
            }
        }
        proofs.push(ProjectionProof {
            name,
            hashes: std::sync::Arc::from(hashes),
        });
    }
    Ok(proofs)
}
