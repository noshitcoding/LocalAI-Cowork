//! Select renderer content from fully verified typed rows and fixed original schemas.
//! No original path is opened; credentials, ACLs and execution state stay in quarantine.
use super::types::{
    HistoricalContentArtifact as ArchivedProductArtifact,
    HistoricalContentMessage as ArchivedProductMessage,
};
use super::{
    types::*,
    verifier::{HistoricalCell, VerifiedHistoryArchive},
};
use crate::{Error, Result, hash, id, json_string};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

type Fields = BTreeMap<String, String>;
fn field<'a>(r: &'a Fields, k: &str) -> &'a str {
    r.get(k).map(String::as_str).unwrap_or("")
}
fn text(v: &str, max: usize) -> Result<String> {
    if v.len() > max || v.contains('\0') {
        return Err(Error::Validation(
            "Projected historical content exceeds current content limits".into(),
        ));
    }
    Ok(v.to_owned())
}
fn name(v: &str, default: &str, max: usize) -> Result<String> {
    text(
        if v.trim().is_empty() {
            default
        } else {
            v.trim()
        },
        max,
    )
}
fn order(source: &str, kind: &str, title: &str, draft: &str) -> Result<HistoricalContentOrder> {
    Ok(HistoricalContentOrder {
        source_id: text(source, 300)?,
        source_kind: kind.into(),
        title: name(title, "Importierter Auftrag", 200)?,
        draft: text(draft, 128 * 1024)?,
        messages: vec![],
        artifacts: vec![],
    })
}
fn project(source: &str, name_: &str) -> Result<HistoricalContentProject> {
    Ok(HistoricalContentProject {
        source_id: text(source, 300)?,
        name: name(name_, "Importiertes Projekt", 200)?,
        orders: vec![],
        documents: vec![],
        files: vec![],
    })
}
fn allowed(table: &str, column: &str) -> bool {
    match table {
        "projects" => matches!(column, "id" | "title" | "instructions"),
        "project_threads" => matches!(column, "project_id" | "thread_id"),
        "chat_threads" => matches!(column, "id" | "title"),
        "chat_messages" => matches!(
            column,
            "id" | "thread_id" | "role" | "content" | "timestamp"
        ),
        "tasks" | "work_tasks" => {
            matches!(column, "id" | "thread_id" | "title" | "prompt" | "output")
        }
        "task_steps" => matches!(
            column,
            "id" | "task_id" | "idx" | "title" | "state" | "output"
        ),
        "engine_runs" => matches!(
            column,
            "id" | "thread_id" | "title" | "input_summary" | "result_summary"
        ),
        "artifact_versions" => matches!(
            column,
            "id" | "run_id"
                | "label"
                | "format"
                | "summary"
                | "preview"
                | "source_path"
                | "size_bytes"
                | "created_at"
        ),
        "engine_run_artifacts" => matches!(
            column,
            "id" | "run_id" | "kind" | "title" | "summary" | "path" | "created_at"
        ),
        "project_resources" => matches!(column, "id" | "project_id" | "kind" | "path" | "label"),
        "skills" => matches!(column, "id" | "name" | "description" | "prompt_template"),
        "agent_personalities" => matches!(
            column,
            "id" | "name" | "description" | "system_prompt" | "skills_markdown"
        ),
        "daemon_entities" => matches!(column, "entity_type" | "id" | "payload_json" | "tombstone"),
        "daemon_runs" => matches!(column, "id" | "thread_id" | "record_json"),
        "daemon_run_events" => matches!(column, "run_id" | "sequence" | "event_json"),
        _ => false,
    }
}
fn message(r: &Fields) -> Result<ArchivedProductMessage> {
    let role = field(r, "role");
    if !matches!(role, "system" | "user" | "assistant" | "tool") {
        return Err(Error::Conflict);
    }
    let raw = field(r, "content");
    let mut content = raw.to_owned();
    if raw.trim_start().starts_with('{') && raw.contains("open-cowork-chat-message") {
        let v: Value = serde_json::from_str(raw).map_err(|_| Error::Conflict)?;
        if v["kind"] != "open-cowork-chat-message"
            || v["version"] != 1
            || v["message"]["id"] != field(r, "id")
            || v["message"]["role"] != role
        {
            return Err(Error::Conflict);
        }
        content = v["message"]["content"].as_str().unwrap_or("").to_owned();
    }
    Ok(ArchivedProductMessage {
        id: id(),
        source_id: Some(text(field(r, "id"), 300)?),
        role: role.into(),
        content: text(&content, 1024 * 1024)?,
        created_at: field(r, "timestamp").parse().unwrap_or(0),
    })
}
fn physical_file(
    source: &str,
    label: &str,
    path: &str,
    size: Option<u64>,
) -> Result<HistoricalContentFile> {
    // Only a display name derived from path text, never a usable path grant.
    let filename = path.rsplit(['/', '\\']).next().unwrap_or("");
    let name = name(
        if label.trim().is_empty() {
            filename
        } else {
            label
        },
        "Historische Datei",
        300,
    )?;
    if name.contains(['/', '\\', '\r', '\n']) {
        return Err(Error::Conflict);
    }
    Ok(HistoricalContentFile {
        source_id: text(source, 300)?,
        name,
        media_type: "application/octet-stream".into(),
        size,
        sha256: None,
        reason: "original_physical_bytes_require_explicit_native_capture".into(),
    })
}
pub(crate) struct ContentStage {
    connection: Option<sqlx::SqliteConnection>,
    directory: Option<tempfile::TempDir>,
    family: String,
}
impl Drop for ContentStage {
    fn drop(&mut self) {
        if let (Some(connection), Some(directory)) = (self.connection.take(), self.directory.take())
        {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    use sqlx::Connection;
                    let _ = connection.close().await;
                    drop(directory);
                });
            } else {
                let _ = directory.keep();
                drop(connection);
            }
        }
    }
}
impl ContentStage {
    pub(crate) async fn unsupported(
        &mut self,
        group: &str,
        reason: &str,
    ) -> Result<HistoricalContentProject> {
        use sqlx::Row;
        let row=sqlx::query("SELECT payload FROM selected_content WHERE project=? AND table_name='projects' LIMIT 1").bind(group).fetch_optional(self.connection.as_mut().ok_or(Error::Conflict)?).await.map_err(crate::storage_error)?;
        let title = if let Some(row) = row {
            let f: Fields =
                serde_json::from_str(&row.try_get::<String, _>(0).map_err(crate::storage_error)?)
                    .map_err(crate::storage_error)?;
            field(&f, "title").to_owned()
        } else {
            "Unveränderte Originalhistorie".to_owned()
        };
        let safe_title = if title.len() > 200 || title.contains('\0') {
            "Unveränderte Originalhistorie"
        } else {
            &title
        };
        let mut value = project(group, safe_title)?;
        value.documents.push(HistoricalContentDocument {
            source_id: format!("retained-history:{group}"),
            title: "Unveränderte Originalhistorie".into(),
            content: reason.into(),
        });
        Ok(value)
    }
    pub(crate) async fn close(mut self) -> Result<()> {
        use sqlx::Connection;
        if let Some(connection) = self.connection.take() {
            connection.close().await.map_err(crate::storage_error)?;
        }
        self.directory.take();
        Ok(())
    }
    pub(crate) async fn groups(&mut self) -> Result<Vec<String>> {
        use sqlx::Row;
        let rows = sqlx::query("SELECT DISTINCT project FROM selected_content ORDER BY project")
            .fetch_all(self.connection.as_mut().ok_or(Error::Conflict)?)
            .await
            .map_err(crate::storage_error)?;
        rows.into_iter()
            .map(|r| r.try_get(0).map_err(crate::storage_error))
            .collect()
    }
    pub(crate) async fn materialize(
        &mut self,
        group: &str,
    ) -> Result<Vec<HistoricalContentProject>> {
        use futures_util::TryStreamExt;
        use sqlx::Row;
        let unsupported:Option<String>=sqlx::query_scalar("SELECT unsupported FROM selected_content WHERE project=? AND unsupported<>'' ORDER BY table_name,position LIMIT 1").bind(group).fetch_optional(self.connection.as_mut().ok_or(Error::Conflict)?).await.map_err(crate::storage_error)?;
        if let Some(reason) = unsupported {
            return Err(Error::Validation(reason));
        }
        let mut rows=sqlx::query("SELECT table_name,payload FROM selected_content WHERE project=? ORDER BY table_name,position LIMIT 1000001").bind(group).fetch(self.connection.as_mut().ok_or(Error::Conflict)?);
        let mut tables: BTreeMap<String, Vec<Fields>> = BTreeMap::new();
        let mut bytes = 0usize;
        while let Some(r) = rows.try_next().await.map_err(crate::storage_error)? {
            let table: String = r.try_get(0).map_err(crate::storage_error)?;
            let raw: String = r.try_get(1).map_err(crate::storage_error)?;
            bytes = bytes.checked_add(raw.len()).ok_or(Error::BudgetExceeded)?;
            if bytes > 256 * 1024 * 1024 {
                return Err(Error::Validation(
                    "One source project needs smaller explicit content partitions".into(),
                ));
            }
            let value = serde_json::from_str(&raw).map_err(crate::storage_error)?;
            tables.entry(table).or_default().push(value);
        }
        normalize_tables(&tables, &self.family)
    }
}
pub(crate) async fn stage_content(
    archive: &VerifiedHistoryArchive,
    private_root: &std::path::Path,
) -> Result<ContentStage> {
    use sqlx::Connection;
    let raw = include_bytes!("verifier/data/profiles.json");
    if hash(raw) != "346fb4292436b1906bacb8d2a91719b12cc85a8331281039de63d75fc859c31f" {
        return Err(Error::Conflict);
    }
    let fixed: Value = serde_json::from_slice(raw).map_err(crate::storage_error)?;
    let profile = fixed["sources"]
        .as_array()
        .ok_or(Error::Conflict)?
        .iter()
        .find(|p| p["profile"] == archive.provenance().source_profile)
        .ok_or(Error::Conflict)?;
    let schemas = profile["schema"]["tables"]
        .as_object()
        .ok_or(Error::Conflict)?;
    let directory = tempfile::Builder::new()
        .prefix("historical-renderer-")
        .tempdir_in(private_root)
        .map_err(crate::storage_error)?;
    let path = directory.path().join("content.sqlite3");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(&path).map_err(crate::storage_error)?;
    let connection = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Memory),
    )
    .await
    .map_err(crate::storage_error)?;
    let mut stage = ContentStage {
        connection: Some(connection),
        directory: Some(directory),
        family: archive.provenance().source_family.clone(),
    };
    let conn = stage.connection.as_mut().ok_or(Error::Conflict)?;
    sqlx::query("CREATE TABLE selected_content(table_name TEXT NOT NULL,position INTEGER NOT NULL,source_id TEXT NOT NULL,project TEXT NOT NULL,thread TEXT NOT NULL,run TEXT NOT NULL,task TEXT NOT NULL,payload TEXT NOT NULL,unsupported TEXT NOT NULL,PRIMARY KEY(table_name,position))").execute(&mut *conn).await.map_err(crate::storage_error)?;
    let mut rows = archive.rows().await.map_err(|_| Error::Conflict)?;
    while let Some(r) = rows.next().await.map_err(|_| Error::Conflict)? {
        let columns = schemas
            .get(&r.source_table)
            .and_then(|s| s["columns"].as_array())
            .ok_or(Error::Conflict)?;
        if r.cells.len() != columns.len() {
            return Err(Error::Conflict);
        }
        let mut fields = Fields::new();
        let mut unsupported = String::new();
        for (cell, col) in r.cells.into_iter().zip(columns) {
            let key = col.get(1).and_then(Value::as_str).ok_or(Error::Conflict)?;
            if !allowed(&r.source_table, key) {
                continue;
            }
            let value = match cell {
                HistoricalCell::TextBytes(v) => match String::from_utf8(v) {
                    Ok(text) => text,
                    Err(_) => {
                        unsupported="Original selected content contains non-UTF-8 text; retain exact raw bytes as history".into();
                        String::new()
                    }
                },
                HistoricalCell::Integer(n) => n.to_string(),
                HistoricalCell::Null => String::new(),
                _ => {
                    unsupported="Original selected content has a non-renderable SQLite cell type; retain exact raw bytes as history".into();
                    String::new()
                }
            };
            fields.insert(key.to_owned(), value);
        }
        if !fields.is_empty() {
            if unsupported.is_empty() {
                match sanitize(&r.source_table,&mut fields) {
                    Ok(false)=>continue,
                    Ok(true)=>{},
                    Err(_)=>unsupported="Original selected content has an unsupported structured renderer payload; retain exact raw bytes as history".into(),
                }
            }
            if !unsupported.is_empty() {
                // Failed structured payloads may contain old credentials. Only
                // their original encrypted archive retains them, never this
                // renderer index. Scalar relationship IDs remain for grouping.
                for field in ["payload_json", "record_json", "event_json"] {
                    fields.remove(field);
                }
            }
            let source = field(&fields, "id").to_owned();
            let project = if r.source_table == "projects" {
                source.clone()
            } else {
                field(&fields, "project_id").to_owned()
            };
            sqlx::query("INSERT INTO selected_content VALUES(?,?,?,?,?,?,?,?,?)")
                .bind(&r.source_table)
                .bind(r.position as i64)
                .bind(source)
                .bind(if project.is_empty() {
                    "unassigned-v1-content"
                } else {
                    &project
                })
                .bind(field(&fields, "thread_id"))
                .bind(field(&fields, "run_id"))
                .bind(field(&fields, "task_id"))
                .bind(json_string(&fields)?)
                .bind(unsupported)
                .execute(&mut *conn)
                .await
                .map_err(crate::storage_error)?;
        }
    }
    rows.close().await.map_err(|_| Error::Conflict)?;
    sqlx::query("CREATE INDEX selected_content_identity ON selected_content(table_name,source_id)")
        .execute(&mut *conn)
        .await
        .map_err(crate::storage_error)?;
    sqlx::query("CREATE INDEX selected_content_thread ON selected_content(table_name,thread)")
        .execute(&mut *conn)
        .await
        .map_err(crate::storage_error)?;
    for (table, parent, column) in [
        ("chat_threads", "project_threads", "thread"),
        ("chat_messages", "chat_threads", "source_id"),
        ("tasks", "chat_threads", "source_id"),
        ("work_tasks", "chat_threads", "source_id"),
        ("engine_runs", "chat_threads", "source_id"),
    ] {
        let q = if table == "chat_threads" {
            format!(
                "UPDATE selected_content SET project=COALESCE((SELECT p.project FROM selected_content p WHERE p.table_name='{parent}' AND p.{column}=selected_content.source_id),'unassigned-v1-content') WHERE table_name='{table}'"
            )
        } else {
            format!(
                "UPDATE selected_content SET project=COALESCE((SELECT p.project FROM selected_content p WHERE p.table_name='{parent}' AND p.{column}=selected_content.thread),'unassigned-v1-content') WHERE table_name='{table}'"
            )
        };
        sqlx::query(&q)
            .execute(&mut *conn)
            .await
            .map_err(crate::storage_error)?;
    }
    for table in ["artifact_versions", "engine_run_artifacts"] {
        let q = format!(
            "UPDATE selected_content SET project=COALESCE((SELECT p.project FROM selected_content p WHERE p.table_name='engine_runs' AND p.source_id=selected_content.run),'unassigned-v1-content') WHERE table_name='{table}'"
        );
        sqlx::query(&q)
            .execute(&mut *conn)
            .await
            .map_err(crate::storage_error)?;
    }
    sqlx::query("UPDATE selected_content SET project=COALESCE((SELECT p.project FROM selected_content p WHERE p.table_name='tasks' AND p.source_id=selected_content.task),'unassigned-v1-content') WHERE table_name='task_steps'").execute(&mut *conn).await.map_err(crate::storage_error)?;
    sqlx::query("UPDATE selected_content SET project=COALESCE((SELECT p.project FROM selected_content p WHERE p.table_name='daemon_entities' AND p.source_id=selected_content.thread AND p.thread=''),'unassigned-v1-content') WHERE table_name='daemon_entities' AND thread!=''").execute(&mut *conn).await.map_err(crate::storage_error)?;
    sqlx::query("UPDATE selected_content SET project=COALESCE((SELECT p.project FROM selected_content p WHERE p.table_name='daemon_runs' AND p.source_id=selected_content.run),'unassigned-v1-content') WHERE table_name='daemon_run_events'").execute(&mut *conn).await.map_err(crate::storage_error)?;
    sqlx::query(
        "CREATE INDEX selected_content_project ON selected_content(project,table_name,position)",
    )
    .execute(&mut *conn)
    .await
    .map_err(crate::storage_error)?;
    Ok(stage)
}
fn sanitize(table: &str, fields: &mut Fields) -> Result<bool> {
    if table == "daemon_entities" {
        if field(fields, "tombstone") != "0"
            || !matches!(
                field(fields, "entity_type"),
                "project" | "thread" | "message" | "task"
            )
        {
            return Ok(false);
        }
        let v: Value =
            serde_json::from_str(field(fields, "payload_json")).map_err(crate::storage_error)?;
        let mut safe = serde_json::Map::new();
        for k in [
            "id",
            "name",
            "title",
            "project_id",
            "thread_id",
            "role",
            "content",
            "description",
            "prompt",
        ] {
            if let Some(v) = v.get(k) {
                safe.insert(k.into(), v.clone());
            }
        }
        let kind = field(fields, "entity_type").to_owned();
        let project = if kind == "project" {
            field(fields, "id").to_owned()
        } else {
            v["project_id"].as_str().unwrap_or("").to_owned()
        };
        fields.insert("project_id".into(), project);
        if kind == "message" || kind == "task" {
            fields.insert(
                "thread_id".into(),
                v["thread_id"].as_str().unwrap_or("").into(),
            );
        }
        fields.insert("payload_json".into(), Value::Object(safe).to_string());
    } else if table == "daemon_runs" {
        let v: Value =
            serde_json::from_str(field(fields, "record_json")).map_err(crate::storage_error)?;
        let spec = &v["spec"];
        let safe = serde_json::json!({"spec":{"id":spec["id"],"thread_id":spec["thread_id"],"project_id":spec["project_id"],"input":{"prompt":spec["input"]["prompt"],"messages":spec["input"]["messages"]}},"result":{"content":v["result"]["content"]}});
        fields.insert(
            "project_id".into(),
            spec["project_id"].as_str().ok_or(Error::Conflict)?.into(),
        );
        fields.insert("record_json".into(), safe.to_string());
    } else if table == "daemon_run_events" {
        let v: Value =
            serde_json::from_str(field(fields, "event_json")).map_err(crate::storage_error)?;
        if !matches!(
            v["kind"].as_str(),
            Some("model_completed" | "artifact_created")
        ) {
            return Ok(false);
        }
        let safe = serde_json::json!({"run_id":v["run_id"],"kind":v["kind"],"payload":{"content":v["payload"]["content"],"storage":v["payload"]["storage"],"path":v["payload"]["path"]}});
        fields.insert("event_json".into(), safe.to_string());
    }
    Ok(true)
}
fn normalize_tables(
    tables: &BTreeMap<String, Vec<Fields>>,
    family: &str,
) -> Result<Vec<HistoricalContentProject>> {
    let empty = Vec::new();
    let get = |table: &str| tables.get(table).unwrap_or(&empty);
    let mut projects = BTreeMap::new();
    for r in get("projects") {
        let mut p = project(field(r, "id"), field(r, "title"))?;
        if !field(r, "instructions").is_empty() {
            p.documents.push(HistoricalContentDocument {
                source_id: format!("project-instructions:{}", p.source_id),
                title: "Projektanweisungen".into(),
                content: text(field(r, "instructions"), 128 * 1024)?,
            });
        }
        if projects.insert(p.source_id.clone(), p).is_some() {
            return Err(Error::Conflict);
        }
    }
    let unassigned = "unassigned-v1-content".to_owned();
    projects.insert(
        unassigned.clone(),
        project(&unassigned, "Weitere importierte Inhalte")?,
    );
    let mut links = BTreeMap::new();
    for r in get("project_threads") {
        let p = field(r, "project_id");
        if !projects.contains_key(p)
            || links
                .insert(field(r, "thread_id").to_owned(), p.to_owned())
                .is_some()
        {
            return Err(Error::Conflict);
        }
    }
    let mut threads: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for r in get("chat_threads") {
        let dest = links.get(field(r, "id")).unwrap_or(&unassigned).clone();
        let p = projects.get_mut(&dest).ok_or(Error::Conflict)?;
        let idx = p.orders.len();
        p.orders
            .push(order(field(r, "id"), "chat_thread", field(r, "title"), "")?);
        if threads.insert(field(r, "id").into(), (dest, idx)).is_some() {
            return Err(Error::Conflict);
        }
    }
    for r in get("chat_messages") {
        let (p, i) = threads.get(field(r, "thread_id")).ok_or(Error::Conflict)?;
        let target = &mut projects.get_mut(p).ok_or(Error::Conflict)?.orders[*i];
        let m = message(r)?;
        if target.messages.len() >= 10000 {
            return Err(Error::BudgetExceeded);
        }
        target.messages.push(m);
    }
    // An editable draft contains original user text, without tool/approval state.
    for p in projects.values_mut() {
        for o in &mut p.orders {
            o.draft = text(
                o.messages
                    .iter()
                    .rev()
                    .find(|m| m.role == "user")
                    .map(|m| m.content.as_str())
                    .unwrap_or(""),
                128 * 1024,
            )?;
        }
    }
    let mut runs = BTreeMap::new();
    for r in get("engine_runs") {
        runs.insert(field(r, "id").to_owned(), field(r, "thread_id").to_owned());
    }
    for table in ["tasks", "work_tasks"] {
        for r in get(table) {
            let dest = links.get(field(r, "thread_id")).unwrap_or(&unassigned);
            let p = projects.get_mut(dest).ok_or(Error::Conflict)?;
            let mut o = order(field(r, "id"), table, field(r, "title"), field(r, "prompt"))?;
            if !field(r, "output").is_empty() {
                o.messages.push(ArchivedProductMessage {
                    id: id(),
                    source_id: Some(format!("output:{}", o.source_id)),
                    role: "assistant".into(),
                    content: text(field(r, "output"), 128 * 1024)?,
                    created_at: 0,
                });
            }
            p.orders.push(o);
        }
    }
    for r in get("task_steps") {
        let task = field(r, "task_id");
        let Some(target) = projects
            .values_mut()
            .flat_map(|p| p.orders.iter_mut())
            .find(|o| {
                matches!(o.source_kind.as_str(), "tasks" | "work_tasks") && o.source_id == task
            })
        else {
            return Err(Error::Validation(
                "Original task-step parent is not supported by a current content mapping".into(),
            ));
        };
        let content = text(
            &format!("{}\n\n{}", field(r, "title"), field(r, "output")),
            1024 * 1024,
        )?;
        target.messages.push(ArchivedProductMessage {
            id: id(),
            source_id: Some(format!("task-step:{}", field(r, "id"))),
            role: "assistant".into(),
            content,
            created_at: 0,
        });
    }
    for r in get("engine_runs") {
        let dest = links.get(field(r, "thread_id")).unwrap_or(&unassigned);
        let mut content = order(
            field(r, "id"),
            "engine_run_content",
            field(r, "title"),
            field(r, "input_summary"),
        )?;
        if !field(r, "result_summary").is_empty() {
            content.messages.push(ArchivedProductMessage {
                id: id(),
                source_id: Some(format!("engine-result:{}", field(r, "id"))),
                role: "assistant".into(),
                content: text(field(r, "result_summary"), 1024 * 1024)?,
                created_at: 0,
            });
        }
        projects
            .get_mut(dest)
            .ok_or(Error::Conflict)?
            .orders
            .push(content);
    }
    for table in ["artifact_versions", "engine_run_artifacts"] {
        for r in get(table) {
            let run = field(r, "run_id");
            let thread = runs.get(run).and_then(|t| threads.get(t));
            let (dest, idx) = if let Some((p, i)) = thread {
                (p.clone(), *i)
            } else {
                let p = projects.get_mut(&unassigned).ok_or(Error::Conflict)?;
                if p.orders
                    .last()
                    .is_none_or(|o| o.source_kind != "artifact_history")
                {
                    p.orders.push(order(
                        "embedded-artifact-history",
                        "artifact_history",
                        "Historische Artefakttexte",
                        "",
                    )?);
                }
                (unassigned.clone(), p.orders.len() - 1)
            };
            let content = text(
                &format!("{}\n\n{}", field(r, "summary"), field(r, "preview")),
                crate::productivity_archive::PRODUCTIVITY_ARCHIVE_MAX_BYTES,
            )?;
            let p = projects.get_mut(&dest).ok_or(Error::Conflict)?;
            let label = if table == "artifact_versions" {
                field(r, "label")
            } else {
                field(r, "title")
            };
            p.orders[idx].artifacts.push(ArchivedProductArtifact {
                id: id(),
                source_id: Some(field(r, "id").into()),
                name: name(label, "Historischer Artefakttext", 300)?,
                media_type: "text/plain".into(),
                size: content.len() as u64,
                sha256: hash(content.as_bytes()),
                content,
                created_at: field(r, "created_at").parse().unwrap_or(0),
            });
            let path = if table == "artifact_versions" {
                field(r, "source_path")
            } else {
                field(r, "path")
            };
            if !path.is_empty() {
                p.files.push(physical_file(
                    &format!("{table}:{}", field(r, "id")),
                    label,
                    path,
                    field(r, "size_bytes").parse().ok(),
                )?);
            }
        }
    }
    for r in get("project_resources") {
        if field(r, "kind") == "file" {
            let p = projects
                .get_mut(field(r, "project_id"))
                .ok_or(Error::Conflict)?;
            p.files.push(physical_file(
                &format!("project-resource:{}", field(r, "id")),
                field(r, "label"),
                field(r, "path"),
                None,
            )?);
        }
    }
    for (table, col) in [
        ("skills", "prompt_template"),
        ("agent_personalities", "system_prompt"),
    ] {
        for r in get(table) {
            let content = text(
                &format!(
                    "{}\n\n{}\n\n{}",
                    field(r, "description"),
                    field(r, col),
                    if table == "agent_personalities" {
                        field(r, "skills_markdown")
                    } else {
                        ""
                    }
                ),
                128 * 1024,
            )?;
            projects
                .get_mut(&unassigned)
                .ok_or(Error::Conflict)?
                .documents
                .push(HistoricalContentDocument {
                    source_id: format!("{table}:{}", field(r, "id")),
                    title: name(field(r, "name"), "Historische Anweisungen", 200)?,
                    content,
                });
        }
    }
    if family == "daemon" {
        daemon_content(&tables, &mut projects, &unassigned)?;
    }
    let mut result = vec![];
    for (key, p) in projects {
        if key != unassigned
            || !p.orders.is_empty()
            || !p.documents.is_empty()
            || !p.files.is_empty()
        {
            result.push(p)
        }
    }
    let mut ids = BTreeSet::new();
    for p in &result {
        for order in &p.orders {
            if order.messages.len() > 10000
                || order.artifacts.len() > 1000
                || json_string(&(&order.messages, &order.artifacts))?.len()
                    > crate::productivity_archive::PRODUCTIVITY_ARCHIVE_MAX_BYTES - 4096
            {
                return Err(Error::Validation(
                    "One original thread needs explicit smaller current history partitions".into(),
                ));
            }
            for original in order
                .messages
                .iter()
                .filter_map(|m| m.source_id.as_ref())
                .chain(order.artifacts.iter().filter_map(|a| a.source_id.as_ref()))
            {
                if original.is_empty() || original.len() > 200 || original.contains('\0') {
                    return Err(Error::Validation(
                        "An original history locator exceeds the current content format".into(),
                    ));
                }
            }
        }
        for f in &p.files {
            if !ids.insert(&f.source_id) {
                return Err(Error::Conflict);
            }
        }
    }
    Ok(result)
}
fn daemon_content(
    tables: &BTreeMap<String, Vec<Fields>>,
    projects: &mut BTreeMap<String, HistoricalContentProject>,
    unassigned: &str,
) -> Result<()> {
    let empty = Vec::new();
    let get = |t: &str| tables.get(t).unwrap_or(&empty);
    let mut threads: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for r in get("daemon_entities") {
        if field(r, "tombstone") != "0" || field(r, "entity_type") != "project" {
            continue;
        }
        let v: Value =
            serde_json::from_str(field(r, "payload_json")).map_err(|_| Error::Conflict)?;
        if v.get("id").is_some_and(|id| id != field(r, "id")) {
            return Err(Error::Conflict);
        }
        let p = project(
            field(r, "id"),
            v["name"]
                .as_str()
                .or_else(|| v["title"].as_str())
                .unwrap_or("Historisches Daemonprojekt"),
        )?;
        if projects.insert(p.source_id.clone(), p).is_some() {
            return Err(Error::Conflict);
        }
    }
    for r in get("daemon_entities") {
        if field(r, "tombstone") != "0" || field(r, "entity_type") != "thread" {
            continue;
        }
        let v: Value =
            serde_json::from_str(field(r, "payload_json")).map_err(|_| Error::Conflict)?;
        let dest = v["project_id"].as_str().unwrap_or(unassigned).to_owned();
        let p = projects.get_mut(&dest).ok_or(Error::Conflict)?;
        let idx = p.orders.len();
        p.orders.push(order(
            field(r, "id"),
            "chat_thread",
            v["title"].as_str().unwrap_or("Historischer Chat"),
            "",
        )?);
        threads.insert(field(r, "id").into(), (dest, idx));
    }
    for r in get("daemon_entities") {
        if field(r, "tombstone") != "0" {
            continue;
        }
        let v: Value =
            serde_json::from_str(field(r, "payload_json")).map_err(|_| Error::Conflict)?;
        if field(r, "entity_type") == "message" {
            let (p, i) = threads
                .get(v["thread_id"].as_str().ok_or(Error::Conflict)?)
                .ok_or(Error::Conflict)?;
            let role = v["role"].as_str().ok_or(Error::Conflict)?;
            if !matches!(role, "system" | "user" | "assistant" | "tool") {
                return Err(Error::Conflict);
            }
            projects.get_mut(p).ok_or(Error::Conflict)?.orders[*i]
                .messages
                .push(ArchivedProductMessage {
                    id: id(),
                    source_id: Some(field(r, "id").into()),
                    role: role.into(),
                    content: text(v["content"].as_str().ok_or(Error::Conflict)?, 1024 * 1024)?,
                    created_at: 0,
                });
        } else if field(r, "entity_type") == "task" {
            let p = v["project_id"].as_str().unwrap_or(unassigned);
            projects
                .get_mut(p)
                .ok_or(Error::Conflict)?
                .orders
                .push(order(
                    field(r, "id"),
                    "daemon_task",
                    v["title"].as_str().unwrap_or("Historischer Auftrag"),
                    v["description"]
                        .as_str()
                        .or_else(|| v["prompt"].as_str())
                        .unwrap_or(""),
                )?);
        }
    }
    for r in get("daemon_runs") {
        let v: Value =
            serde_json::from_str(field(r, "record_json")).map_err(|_| Error::Conflict)?;
        let spec = &v["spec"];
        if spec["id"] != field(r, "id") || spec["thread_id"] != field(r, "thread_id") {
            return Err(Error::Conflict);
        }
        let p = spec["project_id"].as_str().ok_or(Error::Conflict)?;
        if !projects.contains_key(p) {
            projects.insert(p.into(), project(p, "Historisches Daemonprojekt")?);
        }
        let mut o = order(
            field(r, "id"),
            "daemon_run_content",
            "Importierter Auftrag",
            spec["input"]["prompt"].as_str().unwrap_or(""),
        )?;
        if let Some(inputs) = spec["input"]["messages"].as_array() {
            for (i, m) in inputs.iter().enumerate() {
                let role = m["role"].as_str().unwrap_or("");
                if matches!(role, "system" | "user" | "assistant" | "tool") {
                    o.messages.push(ArchivedProductMessage {
                        id: id(),
                        source_id: Some(format!("daemon-input:{}:{i}", o.source_id)),
                        role: role.into(),
                        content: text(m["content"].as_str().unwrap_or(""), 1024 * 1024)?,
                        created_at: 0,
                    });
                }
            }
        }
        for event in get("daemon_run_events")
            .iter()
            .filter(|e| field(e, "run_id") == field(r, "id"))
        {
            let e: Value =
                serde_json::from_str(field(event, "event_json")).map_err(|_| Error::Conflict)?;
            if e["run_id"] != field(r, "id") {
                return Err(Error::Conflict);
            }
            if e["kind"] == "model_completed" {
                if let Some(content) = e["payload"]["content"].as_str() {
                    o.messages.push(ArchivedProductMessage {
                        id: id(),
                        source_id: Some(format!(
                            "daemon-event:{}:{}",
                            o.source_id,
                            field(event, "sequence")
                        )),
                        role: "assistant".into(),
                        content: text(content, 1024 * 1024)?,
                        created_at: 0,
                    });
                }
            }
            if e["kind"] == "artifact_created" && e["payload"]["storage"] == "project_workspace" {
                if let Some(path) = e["payload"]["path"].as_str() {
                    projects
                        .get_mut(p)
                        .ok_or(Error::Conflict)?
                        .files
                        .push(physical_file(
                            &format!(
                                "daemon-artifact:{}:{}",
                                o.source_id,
                                field(event, "sequence")
                            ),
                            "",
                            path,
                            None,
                        )?);
                }
            }
        }
        if let Some(content) = v["result"]["content"].as_str() {
            if !o
                .messages
                .iter()
                .any(|m| m.role == "assistant" && m.content == content)
            {
                o.messages.push(ArchivedProductMessage {
                    id: id(),
                    source_id: Some(format!("daemon-result:{}", o.source_id)),
                    role: "assistant".into(),
                    content: text(content, 1024 * 1024)?,
                    created_at: 0,
                });
            }
        }
        projects.get_mut(p).ok_or(Error::Conflict)?.orders.push(o);
    }
    Ok(())
}
