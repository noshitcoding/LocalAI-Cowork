//! Select renderer content from fully verified typed rows and fixed original schemas.
//! No original path is opened; credentials, ACLs and execution state stay in quarantine.
use super::types::{
    HistoricalContentArtifact as ArchivedProductArtifact,
    HistoricalContentMessage as ArchivedProductMessage,
};
use super::{types::*, verifier::VerifiedHistoryArchive};
use crate::{Error, Result, hash, json_string};
use serde_json::Value;
use sqlx::Row;
use std::collections::BTreeMap;

#[path = "content_row.rs"]
mod content_row;

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
fn physical_file(
    source: &str,
    label: &str,
    path: &str,
    size: Option<u64>,
) -> Result<HistoricalContentFile> {
    // Only a display name derived from path text, never a usable path grant.
    let filename = path.rsplit(['/', '\\']).next().unwrap_or("");
    let name = display_name(
        if label.trim().is_empty() {
            filename
        } else {
            label
        },
        "Historische Datei",
        300,
    );
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
        let raw: String = sqlx::query_scalar("SELECT payload FROM content_parts WHERE source_id=?")
            .bind(group)
            .fetch_one(self.connection.as_mut().ok_or(Error::Conflict)?)
            .await
            .map_err(crate::storage_error)?;
        let mut p: HistoricalContentProject =
            serde_json::from_str(&raw).map_err(crate::storage_error)?;
        p.orders.clear();
        p.files.clear();
        p.documents.push(HistoricalContentDocument {
            source_id: format!("retained-history:{group}"),
            title: "Unveränderte Originalhistorie".into(),
            content: reason.into(),
        });
        Ok(p)
    }
    pub(crate) async fn close(mut self) -> Result<()> {
        use sqlx::Connection;
        if let Some(connection) = self.connection.take() {
            connection.close().await.map_err(crate::storage_error)?;
        }
        self.directory.take();
        Ok(())
    }
    pub(crate) async fn next_group(&mut self, after: Option<&str>) -> Result<Option<String>> {
        if let Some(after) = after {
            sqlx::query_scalar(
                "SELECT source_id FROM content_parts WHERE source_id>? ORDER BY source_id LIMIT 1",
            )
            .bind(after)
            .fetch_optional(self.connection.as_mut().ok_or(Error::Conflict)?)
            .await
            .map_err(crate::storage_error)
        } else {
            sqlx::query_scalar("SELECT source_id FROM content_parts ORDER BY source_id LIMIT 1")
                .fetch_optional(self.connection.as_mut().ok_or(Error::Conflict)?)
                .await
                .map_err(crate::storage_error)
        }
    }
    pub(crate) async fn materialize(
        &mut self,
        group: &str,
    ) -> Result<Vec<HistoricalContentProject>> {
        let r = sqlx::query("SELECT payload,unsupported FROM content_parts WHERE source_id=?")
            .bind(group)
            .fetch_one(self.connection.as_mut().ok_or(Error::Conflict)?)
            .await
            .map_err(crate::storage_error)?;
        let reason: String = r.try_get("unsupported").map_err(crate::storage_error)?;
        if !reason.is_empty() {
            return Err(Error::Validation(reason));
        }
        let raw: String = r.try_get("payload").map_err(crate::storage_error)?;
        if raw.len() > MAX_PROJECTED_BYTES {
            return Err(Error::BudgetExceeded);
        }
        Ok(vec![
            serde_json::from_str(&raw).map_err(crate::storage_error)?,
        ])
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
    let row_path = stage
        .directory
        .as_ref()
        .ok_or(Error::Conflict)?
        .path()
        .join("verified-original-row.json");
    let mut row_options = std::fs::OpenOptions::new();
    row_options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        row_options.mode(0o600);
    }
    let mut row_file = row_options.open(row_path).map_err(crate::storage_error)?;
    let mut rows = archive.browser_rows().await.map_err(|_| Error::Conflict)?;
    while let Some(r) = rows.next().await.map_err(|_| Error::Conflict)? {
        use std::io::{Seek, SeekFrom};
        row_file.set_len(0).map_err(crate::storage_error)?;
        row_file
            .seek(SeekFrom::Start(0))
            .map_err(crate::storage_error)?;
        rows.write_current_to(&mut row_file)
            .await
            .map_err(|_| Error::Conflict)?;
        if row_file.metadata().map_err(crate::storage_error)?.len() != r.canonical_bytes {
            return Err(Error::Conflict);
        }
        row_file
            .seek(SeekFrom::Start(0))
            .map_err(crate::storage_error)?;
        let columns = schemas
            .get(&r.source_table)
            .and_then(|s| s["columns"].as_array())
            .ok_or(Error::Conflict)?
            .iter()
            .map(|c| {
                c.get(1)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or(Error::Conflict)
            })
            .collect::<Result<Vec<_>>>()?;
        let (mut fields, mut unsupported) = content_row::selected(
            std::io::BufReader::new(&mut row_file),
            &r.source_table,
            &columns,
            r.source_rowid,
        )?;
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
    row_file.set_len(0).map_err(crate::storage_error)?;
    drop(row_file);
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
    let family = stage.family.clone();
    build_partitions(stage.connection.as_mut().ok_or(Error::Conflict)?, &family).await?;
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
// Each body remains below the existing encrypted page/answer limits. No whole
// project is assembled: only one row, one history segment and one part are held.
const PART_BYTES: usize = 8 * 1024 * 1024;
const TEXT_BYTES: usize = 128 * 1024;
const MESSAGE_BYTES: usize = 1024 * 1024;
fn display_name(v: &str, default: &str, max: usize) -> String {
    let v = v.trim();
    if v.is_empty() || v.len() > max || v.contains('\0') {
        default.into()
    } else {
        v.into()
    }
}
fn locator(
    table: &str,
    position: i64,
    column: &str,
    start: usize,
    end: usize,
    sha: &str,
) -> String {
    format!("m9:{table}:{position}:{column}:{start}-{end}:{sha}")
}
fn stable_id(source: &str) -> String {
    let h = hash(source.as_bytes());
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}
struct TextPieces<'a> {
    value: &'a str,
    offset: usize,
    limit: usize,
}
impl<'a> TextPieces<'a> {
    fn new(value: &'a str, limit: usize) -> Result<Self> {
        if value.contains('\0') {
            return Err(Error::Validation(
                "Original renderer field contains NUL; retain that row as exact history".into(),
            ));
        }
        Ok(Self {
            value,
            offset: 0,
            limit,
        })
    }
}
impl<'a> Iterator for TextPieces<'a> {
    type Item = (usize, usize, &'a str);
    fn next(&mut self) -> Option<Self::Item> {
        if self.offset == self.value.len() {
            return None;
        }
        let start = self.offset;
        let mut end = (start + self.limit).min(self.value.len());
        while !self.value.is_char_boundary(end) {
            end -= 1;
        }
        self.offset = end;
        Some((start, end, &self.value[start..end]))
    }
}
struct HistoryPart {
    table: String,
    position: i64,
    sha: String,
    kind: String,
    title: String,
    draft: String,
    segment: u32,
    order: HistoricalContentOrder,
    bytes: usize,
}
impl HistoryPart {
    fn new(
        table: &str,
        position: i64,
        fields: &Fields,
        kind: &str,
        title: &str,
        draft: &str,
    ) -> Result<Self> {
        let sha = hash(json_string(fields)?.as_bytes());
        let kind = format!("{kind}_content_part");
        let title = display_name(title, "Historischer Inhaltsbereich", 160);
        let mut h = Self {
            table: table.into(),
            position,
            sha,
            kind,
            title,
            draft: draft.into(),
            segment: 0,
            order: HistoricalContentOrder {
                source_id: String::new(),
                source_kind: String::new(),
                title: String::new(),
                draft: String::new(),
                messages: vec![],
                artifacts: vec![],
            },
            bytes: 0,
        };
        h.reset()?;
        Ok(h)
    }
    fn reset(&mut self) -> Result<()> {
        self.order = HistoricalContentOrder {
            source_id: locator(
                &self.table,
                self.position,
                "history",
                self.segment as usize,
                self.segment as usize,
                &self.sha,
            ),
            source_kind: self.kind.clone(),
            title: format!("{} – Bereich {}", self.title, self.segment + 1),
            draft: if self.segment == 0 {
                self.draft.clone()
            } else {
                String::new()
            },
            messages: vec![],
            artifacts: vec![],
        };
        self.bytes = json_string(&self.order)?.len();
        Ok(())
    }
}
struct PartBuilder<'a> {
    conn: &'a mut sqlx::SqliteConnection,
    family: String,
    group: String,
    name: String,
    index: u32,
    part: HistoricalContentProject,
    bytes: usize,
    history: Option<HistoryPart>,
}
impl<'a> PartBuilder<'a> {
    fn new(
        conn: &'a mut sqlx::SqliteConnection,
        family: &str,
        group: &str,
        title: &str,
    ) -> Result<Self> {
        let mut b = Self {
            conn,
            family: family.into(),
            group: group.into(),
            name: display_name(title, "Importiertes Projekt", 160),
            index: 0,
            part: project("placeholder", "Importiertes Projekt")?,
            bytes: 0,
            history: None,
        };
        b.reset()?;
        Ok(b)
    }
    fn reset(&mut self) -> Result<()> {
        let lineage =
            hash(json_string(&("m9-project-part/v1", &self.family, &self.group))?.as_bytes());
        let source = format!("m9-project-part:{lineage}:{:08}", self.index);
        self.part = project(
            &source,
            &format!("{} – Inhaltsbereich {}", self.name, self.index + 1),
        )?;
        self.part.documents.push(HistoricalContentDocument{source_id:format!("m9-part-provenance:{lineage}:{:08}",self.index),title:"Originalherkunft dieses Inhaltsbereichs".into(),content:json_string(&serde_json::json!({"format":"m9-content-part/1","source_family":self.family,"original_project_id":self.group,"part_index":self.index,"locators":"m9:original_table:original_position:field:start-end:complete_field_sha256; UTF-8 byte ranges are half-open","history":"content only; original runs, rights, grants and schedules remain in quarantine","continuations":"Only the first history segment may carry an unchanged complete bounded draft. Continuation drafts are empty."}))?});
        self.bytes = json_string(&self.part)?.len();
        Ok(())
    }
    fn finish_segment(&mut self) -> Result<()> {
        if let Some(mut h) = self.history.take() {
            if h.segment == 0 || !h.order.messages.is_empty() || !h.order.artifacts.is_empty() {
                self.bytes = self
                    .bytes
                    .checked_add(h.bytes)
                    .ok_or(Error::BudgetExceeded)?;
                let order = std::mem::replace(
                    &mut h.order,
                    HistoricalContentOrder {
                        source_id: String::new(),
                        source_kind: String::new(),
                        title: String::new(),
                        draft: String::new(),
                        messages: vec![],
                        artifacts: vec![],
                    },
                );
                self.part.orders.push(order);
                h.segment = h.segment.checked_add(1).ok_or(Error::BudgetExceeded)?;
                h.reset()?;
            }
            self.history = Some(h);
        }
        Ok(())
    }
    async fn flush(&mut self, unsupported: &str) -> Result<()> {
        self.finish_segment()?;
        let bytes = json_string(&self.part)?;
        if bytes.len() > MAX_PROJECTED_BYTES {
            return Err(Error::BudgetExceeded);
        }
        sqlx::query("INSERT INTO content_parts(source_id,original_group,part_index,payload,unsupported) VALUES(?,?,?,?,?)")
            .bind(&self.part.source_id).bind(&self.group).bind(i64::from(self.index)).bind(bytes).bind(unsupported).execute(&mut *self.conn).await.map_err(crate::storage_error)?;
        self.index = self.index.checked_add(1).ok_or(Error::BudgetExceeded)?;
        self.reset()?;
        Ok(())
    }
    async fn room(&mut self, n: usize, document: bool, file: bool) -> Result<()> {
        let history_bytes = self.history.as_ref().map_or(0, |h| h.bytes);
        if n > PART_BYTES {
            return Err(Error::BudgetExceeded);
        }
        if self
            .bytes
            .checked_add(history_bytes)
            .and_then(|v| v.checked_add(n))
            .is_none_or(|v| v > PART_BYTES)
            || (document && self.part.documents.len() >= 1900)
            || (file && self.part.files.len() >= 1900)
            || self.part.orders.len() >= 900
        {
            self.flush("").await?;
        }
        Ok(())
    }
    async fn document(
        &mut self,
        table: &str,
        pos: i64,
        column: &str,
        title: &str,
        value: &str,
    ) -> Result<()> {
        let sha = hash(value.as_bytes());
        for (start, end, piece) in TextPieces::new(value, TEXT_BYTES)? {
            let doc = HistoricalContentDocument {
                source_id: locator(table, pos, column, start, end, &sha),
                title: display_name(title, "Historischer Textbereich", 200),
                content: piece.into(),
            };
            let n = json_string(&doc)?.len() + 1;
            self.room(n, true, false).await?;
            self.bytes += n;
            self.part.documents.push(doc);
        }
        Ok(())
    }
    async fn file(&mut self, file: HistoricalContentFile) -> Result<()> {
        let n = json_string(&file)?.len() + 1;
        self.room(n, false, true).await?;
        self.bytes += n;
        self.part.files.push(file);
        Ok(())
    }
    async fn start_history(
        &mut self,
        table: &str,
        pos: i64,
        fields: &Fields,
        kind: &str,
        title: &str,
        draft: &str,
    ) -> Result<()> {
        self.end_history()?;
        let draft = if draft.len() <= TEXT_BYTES && !draft.contains('\0') {
            draft
        } else {
            ""
        };
        self.room(2048 + draft.len() * 6, false, false).await?;
        self.history = Some(HistoryPart::new(table, pos, fields, kind, title, draft)?);
        Ok(())
    }
    fn end_history(&mut self) -> Result<()> {
        if self.history.is_some() {
            self.finish_segment()?;
            self.history = None;
        }
        Ok(())
    }
    async fn message(
        &mut self,
        table: &str,
        pos: i64,
        column: &str,
        role: &str,
        created_at: i64,
        value: &str,
    ) -> Result<()> {
        if !matches!(role, "system" | "user" | "assistant" | "tool") {
            return Err(Error::Conflict);
        }
        let sha = hash(value.as_bytes());
        for (start, end, piece) in TextPieces::new(value, MESSAGE_BYTES)? {
            let source = locator(table, pos, column, start, end, &sha);
            let m = ArchivedProductMessage {
                id: stable_id(&source),
                source_id: Some(source),
                role: role.into(),
                created_at,
                content: piece.into(),
            };
            let n = json_string(&m)?.len() + 1;
            if self.history.as_ref().is_some_and(|h| {
                h.order.messages.len() >= 10000
                    || h.bytes + n
                        > crate::productivity_archive::PRODUCTIVITY_ARCHIVE_MAX_BYTES - 4096
            }) {
                self.finish_segment()?;
            }
            self.room(n, false, false).await?;
            let h = self.history.as_mut().ok_or(Error::Conflict)?;
            h.bytes += n;
            h.order.messages.push(m);
        }
        Ok(())
    }
    async fn artifact(
        &mut self,
        table: &str,
        pos: i64,
        column: &str,
        name: &str,
        created_at: i64,
        value: &str,
    ) -> Result<()> {
        let sha = hash(value.as_bytes());
        for (start, end, piece) in TextPieces::new(value, MESSAGE_BYTES)? {
            let source = locator(table, pos, column, start, end, &sha);
            let a = ArchivedProductArtifact {
                id: stable_id(&source),
                source_id: Some(source),
                name: display_name(name, "Historischer Artefakttext", 300),
                media_type: "text/plain".into(),
                size: piece.len() as u64,
                sha256: hash(piece.as_bytes()),
                content: piece.into(),
                created_at,
            };
            let n = json_string(&a)?.len() + 1;
            if self.history.as_ref().is_some_and(|h| {
                h.order.artifacts.len() >= 1000
                    || h.bytes + n
                        > crate::productivity_archive::PRODUCTIVITY_ARCHIVE_MAX_BYTES - 4096
            }) {
                self.finish_segment()?;
            }
            self.room(n, false, false).await?;
            let h = self.history.as_mut().ok_or(Error::Conflict)?;
            h.bytes += n;
            h.order.artifacts.push(a);
        }
        Ok(())
    }
    async fn unsupported_row(&mut self, table: &str, pos: i64) -> Result<()> {
        // Preserve the continuation of an enclosing original thread/run. The
        // exceptional child is a separate explicit history-only part, and the
        // following siblings retain their original order without another draft.
        self.finish_segment()?;
        let continuation = self.history.take();
        if self.part.orders.len() + self.part.files.len() + self.part.documents.len() > 1 {
            self.flush("").await?;
        }
        self.part.documents.push(HistoricalContentDocument{source_id:format!("m9-original-row:{table}:{pos}"),title:"Unveränderte Originalzeile".into(),content:format!("Original table {table}, position {pos}: this non-renderable row remains complete in the pinned quarantine; no credential, execution authority or partial task is adopted.")});
        self.flush("Original row requires exact retained-history disposition")
            .await?;
        self.history = continuation;
        Ok(())
    }
    async fn finish(mut self) -> Result<()> {
        self.end_history()?;
        if self.index == 0
            || self.part.orders.len() + self.part.files.len() + self.part.documents.len() > 1
        {
            self.flush("").await?;
        }
        Ok(())
    }
}
struct SelectedRow {
    position: i64,
    fields: Fields,
    unsupported: String,
}
async fn next_row(
    conn: &mut sqlx::SqliteConnection,
    group: &str,
    table: &str,
    after: i64,
    column: &str,
    parent: &str,
) -> Result<Option<SelectedRow>> {
    // All column/table choices are fixed source renderer choices, never operator input.
    let sql = if column.is_empty() {
        "SELECT position,payload,unsupported FROM selected_content s WHERE project=? AND table_name=? AND position>? AND NOT EXISTS(SELECT 1 FROM rendered_content r WHERE r.table_name=s.table_name AND r.position=s.position) ORDER BY position LIMIT 1".to_owned()
    } else {
        format!(
            "SELECT position,payload,unsupported FROM selected_content s WHERE project=? AND table_name=? AND position>? AND {column}=? AND NOT EXISTS(SELECT 1 FROM rendered_content r WHERE r.table_name=s.table_name AND r.position=s.position) ORDER BY position LIMIT 1"
        )
    };
    let mut query = sqlx::query(&sql).bind(group).bind(table).bind(after);
    if !column.is_empty() {
        query = query.bind(parent);
    }
    let Some(r) = query
        .fetch_optional(conn)
        .await
        .map_err(crate::storage_error)?
    else {
        return Ok(None);
    };
    Ok(Some(SelectedRow {
        position: r.try_get("position").map_err(crate::storage_error)?,
        fields: serde_json::from_str(
            &r.try_get::<String, _>("payload")
                .map_err(crate::storage_error)?,
        )
        .map_err(crate::storage_error)?,
        unsupported: r.try_get("unsupported").map_err(crate::storage_error)?,
    }))
}
async fn render_document_row(b: &mut PartBuilder<'_>, table: &str, r: &SelectedRow) -> Result<()> {
    let f = &r.fields;
    match table {
        "projects" => {
            b.document(
                table,
                r.position,
                "instructions",
                "Projektanweisungen",
                field(f, "instructions"),
            )
            .await?;
            if !field(f, "title").is_empty() {
                b.document(
                    table,
                    r.position,
                    "title",
                    "Vollständiger Originalprojekttitel",
                    field(f, "title"),
                )
                .await?;
            }
        }
        "skills" | "agent_personalities" => {
            for (col, label) in [
                ("name", "Originalname"),
                ("description", "Originalbeschreibung"),
                ("prompt_template", "Originalanweisungen"),
                ("system_prompt", "Originalanweisungen"),
                ("skills_markdown", "Originalfähigkeiten"),
            ] {
                b.document(table, r.position, col, label, field(f, col))
                    .await?;
            }
        }
        "project_resources" => {
            b.document(
                table,
                r.position,
                "label",
                "Vollständige Originaldateibeschreibung",
                field(f, "label"),
            )
            .await?;
            if field(f, "kind") == "file" {
                b.file(physical_file(
                    &format!("project-resource:{}", field(f, "id")),
                    field(f, "label"),
                    field(f, "path"),
                    None,
                )?)
                .await?;
            }
        }
        _ => {}
    }
    Ok(())
}
async fn mark_row(conn: &mut sqlx::SqliteConnection, table: &str, position: i64) -> Result<()> {
    sqlx::query("INSERT INTO rendered_content(table_name,position) VALUES(?,?)")
        .bind(table)
        .bind(position)
        .execute(conn)
        .await
        .map_err(crate::storage_error)?;
    Ok(())
}
async fn render_child(b: &mut PartBuilder<'_>, table: &str, r: &SelectedRow) -> Result<()> {
    let f = &r.fields;
    match table {
        "chat_messages" => {
            let role = field(f, "role");
            let raw = field(f, "content");
            let date = field(f, "timestamp").parse().unwrap_or(0);
            if raw.trim_start().starts_with('{') && raw.contains("open-cowork-chat-message") {
                let v: Value = serde_json::from_str(raw).map_err(|_| Error::Conflict)?;
                if v["kind"] != "open-cowork-chat-message"
                    || v["version"] != 1
                    || v["message"]["id"] != field(f, "id")
                    || v["message"]["role"] != role
                {
                    return Err(Error::Conflict);
                }
                b.message(
                    table,
                    r.position,
                    "message_content",
                    role,
                    date,
                    v["message"]["content"].as_str().ok_or(Error::Conflict)?,
                )
                .await?;
            } else {
                b.message(table, r.position, "content", role, date, raw)
                    .await?;
            }
        }
        "task_steps" => {
            for column in ["title", "output"] {
                b.message(table, r.position, column, "assistant", 0, field(f, column))
                    .await?;
            }
        }
        "artifact_versions" | "engine_run_artifacts" => {
            let label = if table == "artifact_versions" {
                field(f, "label")
            } else {
                field(f, "title")
            };
            let date = field(f, "created_at").parse().unwrap_or(0);
            for column in ["summary", "preview"] {
                b.artifact(table, r.position, column, label, date, field(f, column))
                    .await?;
            }
            if !label.is_empty() {
                b.document(
                    table,
                    r.position,
                    if table == "artifact_versions" {
                        "label"
                    } else {
                        "title"
                    },
                    "Vollständiger Originalartefakttitel",
                    label,
                )
                .await?;
            }
            let path = if table == "artifact_versions" {
                field(f, "source_path")
            } else {
                field(f, "path")
            };
            if !path.is_empty() {
                b.file(physical_file(
                    &format!("{table}:{}", field(f, "id")),
                    label,
                    path,
                    field(f, "size_bytes").parse().ok(),
                )?)
                .await?;
            }
        }
        "daemon_entities" => {
            let v: Value =
                serde_json::from_str(field(f, "payload_json")).map_err(crate::storage_error)?;
            if field(f, "entity_type") != "message"
                || v.get("id").is_some_and(|v| v != field(f, "id"))
            {
                return Err(Error::Conflict);
            }
            b.message(
                table,
                r.position,
                "content",
                v["role"].as_str().ok_or(Error::Conflict)?,
                0,
                v["content"].as_str().ok_or(Error::Conflict)?,
            )
            .await?;
        }
        "daemon_run_events" => {
            let v: Value =
                serde_json::from_str(field(f, "event_json")).map_err(crate::storage_error)?;
            if v["run_id"] != field(f, "run_id") {
                return Err(Error::Conflict);
            }
            if v["kind"] == "model_completed" {
                if let Some(text) = v["payload"]["content"].as_str() {
                    b.message(table, r.position, "content", "assistant", 0, text)
                        .await?;
                }
            } else if v["kind"] == "artifact_created"
                && v["payload"]["storage"] == "project_workspace"
                && let Some(path) = v["payload"]["path"].as_str()
            {
                b.file(physical_file(
                    &format!(
                        "daemon-artifact:{}:{}",
                        field(f, "run_id"),
                        field(f, "sequence")
                    ),
                    "",
                    path,
                    None,
                )?)
                .await?;
            }
        }
        _ => return Err(Error::Conflict),
    }
    Ok(())
}
async fn consume_child(b: &mut PartBuilder<'_>, table: &str, r: &SelectedRow) -> Result<()> {
    mark_row(b.conn, table, r.position).await?;
    if !r.unsupported.is_empty() {
        return b.unsupported_row(table, r.position).await;
    }
    match render_child(b, table, r).await {
        Ok(()) => Ok(()),
        Err(Error::Validation(_) | Error::Conflict | Error::BudgetExceeded) => {
            b.unsupported_row(table, r.position).await
        }
        Err(e) => Err(e),
    }
}
async fn render_artifacts(b: &mut PartBuilder<'_>, run: &str) -> Result<()> {
    for table in ["artifact_versions", "engine_run_artifacts"] {
        let mut after = -1;
        while let Some(r) = next_row(b.conn, &b.group, table, after, "run", run).await? {
            after = r.position;
            consume_child(b, table, &r).await?;
        }
    }
    Ok(())
}
async fn render_row(b: &mut PartBuilder<'_>, table: &str, r: &SelectedRow) -> Result<()> {
    let f = &r.fields;
    let pos = r.position;
    match table {
        "projects" | "skills" | "agent_personalities" | "project_resources" => {
            render_document_row(b, table, r).await?
        }
        "chat_threads" => {
            b.start_history(table, pos, f, "chat_thread", field(f, "title"), "")
                .await?;
            if !field(f, "title").is_empty() {
                b.document(
                    table,
                    pos,
                    "title",
                    "Vollständiger Originalgesprächstitel",
                    field(f, "title"),
                )
                .await?;
            }
            let mut after = -1;
            while let Some(m) = next_row(
                b.conn,
                &b.group,
                "chat_messages",
                after,
                "thread",
                field(f, "id"),
            )
            .await?
            {
                after = m.position;
                consume_child(b, "chat_messages", &m).await?;
            }
            b.end_history()?;
        }
        "tasks" | "work_tasks" => {
            let prompt = field(f, "prompt");
            b.start_history(table, pos, f, table, field(f, "title"), prompt)
                .await?;
            if prompt.len() > TEXT_BYTES || prompt.contains('\0') {
                b.document(
                    table,
                    pos,
                    "prompt",
                    "Vollständiger Originalauftragstext; kein ausführbarer Teilauftrag",
                    prompt,
                )
                .await?;
            }
            if !field(f, "title").is_empty() {
                b.document(
                    table,
                    pos,
                    "title",
                    "Vollständiger Originalauftragstitel",
                    field(f, "title"),
                )
                .await?;
            }
            b.message(table, pos, "output", "assistant", 0, field(f, "output"))
                .await?;
            if table == "tasks" {
                let mut after = -1;
                while let Some(step) = next_row(
                    b.conn,
                    &b.group,
                    "task_steps",
                    after,
                    "task",
                    field(f, "id"),
                )
                .await?
                {
                    after = step.position;
                    consume_child(b, "task_steps", &step).await?;
                }
            }
            b.end_history()?;
        }
        "engine_runs" => {
            let prompt = field(f, "input_summary");
            b.start_history(
                table,
                pos,
                f,
                "engine_run_content",
                field(f, "title"),
                prompt,
            )
            .await?;
            if prompt.len() > TEXT_BYTES || prompt.contains('\0') {
                b.document(
                    table,
                    pos,
                    "input_summary",
                    "Vollständiger Originalinput; kein ausführbarer Teilauftrag",
                    prompt,
                )
                .await?;
            }
            if !field(f, "title").is_empty() {
                b.document(
                    table,
                    pos,
                    "title",
                    "Vollständiger Originallauftitel",
                    field(f, "title"),
                )
                .await?;
            }
            b.message(
                table,
                pos,
                "result_summary",
                "assistant",
                0,
                field(f, "result_summary"),
            )
            .await?;
            render_artifacts(b, field(f, "id")).await?;
            b.end_history()?;
        }
        "daemon_entities" => {
            let v: Value =
                serde_json::from_str(field(f, "payload_json")).map_err(crate::storage_error)?;
            if v.get("id").is_some_and(|v| v != field(f, "id")) {
                return Err(Error::Conflict);
            }
            match field(f, "entity_type") {
                "project" => {
                    for key in ["name", "title"] {
                        if let Some(title) = v[key].as_str().filter(|s| !s.is_empty()) {
                            b.document(
                                table,
                                pos,
                                key,
                                "Vollständiger Originalprojekttitel",
                                title,
                            )
                            .await?;
                        }
                    }
                }
                "thread" => {
                    b.start_history(
                        table,
                        pos,
                        f,
                        "daemon_thread",
                        v["title"].as_str().unwrap_or("Historisches Gespräch"),
                        "",
                    )
                    .await?;
                    if let Some(title) = v["title"].as_str().filter(|s| !s.is_empty()) {
                        b.document(
                            table,
                            pos,
                            "title",
                            "Vollständiger Originalgesprächstitel",
                            title,
                        )
                        .await?;
                    }
                    let mut after = -1;
                    while let Some(m) =
                        next_row(b.conn, &b.group, table, after, "thread", field(f, "id")).await?
                    {
                        after = m.position;
                        if field(&m.fields, "entity_type") == "message" {
                            consume_child(b, table, &m).await?;
                        }
                    }
                    b.end_history()?;
                }
                "task" => {
                    let prompt = v["description"]
                        .as_str()
                        .or_else(|| v["prompt"].as_str())
                        .unwrap_or("");
                    b.start_history(
                        table,
                        pos,
                        f,
                        "daemon_task",
                        v["title"].as_str().unwrap_or("Historischer Auftrag"),
                        prompt,
                    )
                    .await?;
                    if prompt.len() > TEXT_BYTES || prompt.contains('\0') {
                        b.document(
                            table,
                            pos,
                            if v["description"].is_string() {
                                "description"
                            } else {
                                "prompt"
                            },
                            "Vollständiger Originalauftragstext; kein ausführbarer Teilauftrag",
                            prompt,
                        )
                        .await?;
                    }
                    if let Some(title) = v["title"].as_str().filter(|s| !s.is_empty()) {
                        b.document(
                            table,
                            pos,
                            "title",
                            "Vollständiger Originalauftragstitel",
                            title,
                        )
                        .await?;
                    }
                    b.end_history()?;
                }
                "message" => {
                    b.start_history(
                        table,
                        pos,
                        f,
                        "orphan_original_message",
                        "Historische Originalnachricht",
                        "",
                    )
                    .await?;
                    render_child(b, table, r).await?;
                    b.end_history()?;
                }
                _ => {}
            }
        }
        "daemon_runs" => {
            let v: Value =
                serde_json::from_str(field(f, "record_json")).map_err(crate::storage_error)?;
            let spec = &v["spec"];
            if spec["id"] != field(f, "id") || spec["thread_id"] != field(f, "thread_id") {
                return Err(Error::Conflict);
            }
            let prompt = spec["input"]["prompt"].as_str().unwrap_or("");
            b.start_history(
                table,
                pos,
                f,
                "daemon_run_content",
                "Historischer Laufinhalt",
                prompt,
            )
            .await?;
            if prompt.len() > TEXT_BYTES || prompt.contains('\0') {
                b.document(
                    table,
                    pos,
                    "prompt",
                    "Vollständiger Originalinput; kein ausführbarer Teilauftrag",
                    prompt,
                )
                .await?;
            }
            if let Some(messages) = spec["input"]["messages"].as_array() {
                for (i, m) in messages.iter().enumerate() {
                    b.message(
                        table,
                        pos,
                        &format!("input_{i}"),
                        m["role"].as_str().ok_or(Error::Conflict)?,
                        0,
                        m["content"].as_str().ok_or(Error::Conflict)?,
                    )
                    .await?;
                }
            }
            let mut after = -1;
            while let Some(e) = next_row(
                b.conn,
                &b.group,
                "daemon_run_events",
                after,
                "run",
                field(f, "id"),
            )
            .await?
            {
                after = e.position;
                consume_child(b, "daemon_run_events", &e).await?;
            }
            if let Some(text) = v["result"]["content"].as_str() {
                b.message(table, pos, "result", "assistant", 0, text)
                    .await?;
            }
            b.end_history()?;
        }
        "chat_messages"
        | "task_steps"
        | "artifact_versions"
        | "engine_run_artifacts"
        | "daemon_run_events" => {
            b.start_history(
                table,
                pos,
                f,
                "orphan_original_content",
                "Historischer Originalinhalt",
                "",
            )
            .await?;
            render_child(b, table, r).await?;
            b.end_history()?;
        }
        _ => {}
    }
    Ok(())
}
async fn build_partitions(conn: &mut sqlx::SqliteConnection, family: &str) -> Result<()> {
    sqlx::query("CREATE TABLE rendered_content(table_name TEXT NOT NULL,position INTEGER NOT NULL,PRIMARY KEY(table_name,position))").execute(&mut *conn).await.map_err(crate::storage_error)?;
    sqlx::query("CREATE TABLE content_parts(source_id TEXT PRIMARY KEY,original_group TEXT NOT NULL,part_index INTEGER NOT NULL,payload TEXT NOT NULL,unsupported TEXT NOT NULL,UNIQUE(original_group,part_index))").execute(&mut *conn).await.map_err(crate::storage_error)?;
    sqlx::query("CREATE INDEX selected_content_parent_run ON selected_content(project,table_name,run,position)").execute(&mut *conn).await.map_err(crate::storage_error)?;
    sqlx::query("CREATE INDEX selected_content_parent_task ON selected_content(project,table_name,task,position)").execute(&mut *conn).await.map_err(crate::storage_error)?;
    sqlx::query("CREATE INDEX selected_content_parent_thread ON selected_content(project,table_name,thread,position)").execute(&mut *conn).await.map_err(crate::storage_error)?;
    let mut last: Option<String> = None;
    loop {
        let group: Option<String> = if let Some(last) = &last {
            sqlx::query_scalar("SELECT project FROM selected_content WHERE project>? GROUP BY project ORDER BY project LIMIT 1").bind(last).fetch_optional(&mut *conn).await.map_err(crate::storage_error)?
        } else {
            sqlx::query_scalar(
                "SELECT project FROM selected_content GROUP BY project ORDER BY project LIMIT 1",
            )
            .fetch_optional(&mut *conn)
            .await
            .map_err(crate::storage_error)?
        };
        let Some(group) = group else {
            break;
        };
        last = Some(group.clone());
        let mut title:Option<String>=sqlx::query_scalar("SELECT payload FROM selected_content WHERE project=? AND table_name='projects' ORDER BY position LIMIT 1").bind(&group).fetch_optional(&mut *conn).await.map_err(crate::storage_error)?;
        if title.is_none() {
            let daemon:Option<String>=sqlx::query_scalar("SELECT payload FROM selected_content WHERE project=? AND table_name='daemon_entities' AND source_id=? ORDER BY position LIMIT 1").bind(&group).bind(&group).fetch_optional(&mut *conn).await.map_err(crate::storage_error)?;
            if let Some(raw) = daemon {
                let fields: Fields = serde_json::from_str(&raw).map_err(crate::storage_error)?;
                if field(&fields, "entity_type") == "project" {
                    let value: Value =
                        serde_json::from_str(field(&fields, "payload_json")).unwrap_or(Value::Null);
                    let mut selected = Fields::new();
                    selected.insert(
                        "title".into(),
                        value["name"]
                            .as_str()
                            .or_else(|| value["title"].as_str())
                            .unwrap_or("Importierte Inhalte")
                            .to_owned(),
                    );
                    title = Some(json_string(&selected)?);
                }
            }
        }
        let title = title
            .map(|v| serde_json::from_str::<Fields>(&v).map(|v| field(&v, "title").to_owned()))
            .transpose()
            .map_err(crate::storage_error)?
            .unwrap_or_else(|| "Importierte Inhalte".into());
        let mut b = PartBuilder::new(conn, family, &group, &title)?;
        for table in [
            "projects",
            "chat_threads",
            "tasks",
            "work_tasks",
            "engine_runs",
            "daemon_entities",
            "daemon_runs",
            "skills",
            "agent_personalities",
            "project_resources",
            // Orphaned children, including children of a non-renderable parent,
            // remain editable as inert original-content histories of their own.
            "chat_messages",
            "task_steps",
            "artifact_versions",
            "engine_run_artifacts",
            "daemon_run_events",
        ] {
            let mut after = -1;
            while let Some(r) = next_row(b.conn, &group, table, after, "", "").await? {
                after = r.position;
                if table == "daemon_entities" && field(&r.fields, "entity_type") == "message" {
                    continue;
                }
                mark_row(b.conn, table, r.position).await?;
                if !r.unsupported.is_empty() {
                    b.unsupported_row(table, r.position).await?;
                    continue;
                }
                match render_row(&mut b, table, &r).await {
                    Ok(()) => {}
                    Err(Error::Validation(_) | Error::Conflict | Error::BudgetExceeded) => {
                        b.unsupported_row(table, r.position).await?
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        let mut after = -1;
        while let Some(r) = next_row(b.conn, &group, "daemon_entities", after, "", "").await? {
            after = r.position;
            mark_row(b.conn, "daemon_entities", r.position).await?;
            if !r.unsupported.is_empty() {
                b.unsupported_row("daemon_entities", r.position).await?;
                continue;
            }
            match render_row(&mut b, "daemon_entities", &r).await {
                Ok(()) => {}
                Err(Error::Validation(_) | Error::Conflict | Error::BudgetExceeded) => {
                    b.unsupported_row("daemon_entities", r.position).await?
                }
                Err(e) => return Err(e),
            }
        }
        b.finish().await?;
        let unassigned: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM selected_content s WHERE project=? AND table_name<>'project_threads' AND NOT EXISTS(SELECT 1 FROM rendered_content r WHERE r.table_name=s.table_name AND r.position=s.position)")
            .bind(&group).fetch_one(&mut *conn).await.map_err(crate::storage_error)?;
        if unassigned != 0 {
            return Err(Error::Conflict);
        }
    }
    Ok(())
}
