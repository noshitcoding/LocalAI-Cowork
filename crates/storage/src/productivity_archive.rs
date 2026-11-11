//! Versioned inert V2 content transfer; never transfers credentials or execution authority.
use super::productivity::*;
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read, Write},
};

pub const PRODUCTIVITY_ARCHIVE_MAX_BYTES: usize = 16 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductivityArchive {
    pub format: String,
    pub version: u32,
    pub created_at: i64,
    pub projects: Vec<ArchivedProductProject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub personal_knowledge_folders: Vec<KnowledgeFolder>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivedProductProject {
    pub id: String,
    pub name: String,
    pub revision: i64,
    pub orders: Vec<ArchivedProductOrder>,
    pub skills: Vec<Skill>,
    pub knowledge: Vec<KnowledgeDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowledge_folders: Vec<KnowledgeFolder>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowledge_revisions: Vec<KnowledgeRevision>,
    pub schedules: Vec<ProductSchedule>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivedProductOrder {
    pub id: String,
    pub title: String,
    pub draft: String,
    pub revision: i64,
    #[serde(default)]
    pub messages: Vec<ArchivedProductMessage>,
    #[serde(default)]
    pub artifacts: Vec<ArchivedProductArtifact>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivedProductMessage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    pub role: String,
    pub content: String,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivedProductArtifact {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
    pub content: String,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedProductHistory {
    pub project_id: String,
    pub order_id: String,
    pub messages: Vec<ArchivedProductMessage>,
    pub artifacts: Vec<ArchivedProductArtifact>,
    pub revision: i64,
    pub imported_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProductivityImportPolicy {
    Reject,
    Copy,
    Replace,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductivityImportRequest {
    pub idempotency_key: String,
    pub policy: ProductivityImportPolicy,
    #[serde(default)]
    pub expected_project_revisions: BTreeMap<String, i64>,
    #[serde(default)]
    pub expected_order_revisions: BTreeMap<String, i64>,
    pub archive: ProductivityArchive,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductivityImportedProject {
    pub source_id: String,
    pub project: Project,
    pub order_ids: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductivityImportResult {
    pub version: u32,
    pub projects: Vec<ProductivityImportedProject>,
    pub schedules_paused: usize,
}

fn archive_error() -> Error {
    Error::Validation("Invalid or oversized V2 product archive".into())
}
impl ProductivityArchive {
    pub fn to_zip(&self) -> Result<Vec<u8>> {
        let body = serde_json::to_vec(self).map_err(storage_error)?;
        if body.len() > PRODUCTIVITY_ARCHIVE_MAX_BYTES {
            return Err(archive_error());
        }
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "manifest.json",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .map_err(|_| archive_error())?;
        zip.write_all(&body).map_err(storage_error)?;
        Ok(zip.finish().map_err(|_| archive_error())?.into_inner())
    }
    pub fn from_zip(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > PRODUCTIVITY_ARCHIVE_MAX_BYTES {
            return Err(archive_error());
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| archive_error())?;
        if zip.len() != 1 {
            return Err(archive_error());
        }
        let mut entry = zip.by_index(0).map_err(|_| archive_error())?;
        if entry.name().map_err(|_| archive_error())? != "manifest.json"
            || entry.size() > PRODUCTIVITY_ARCHIVE_MAX_BYTES as u64
            || entry.is_dir()
            || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            return Err(archive_error());
        }
        let mut data = Vec::new();
        entry
            .by_ref()
            .take(PRODUCTIVITY_ARCHIVE_MAX_BYTES as u64 + 1)
            .read_to_end(&mut data)
            .map_err(storage_error)?;
        if data.len() > PRODUCTIVITY_ARCHIVE_MAX_BYTES {
            return Err(archive_error());
        }
        serde_json::from_slice(&data).map_err(|_| archive_error())
    }
    fn validate(&self) -> Result<()> {
        if self.format != "localai-cowork-product"
            || self.version != 1
            || self.projects.len() > 100
            || serde_json::to_vec(self).map_err(storage_error)?.len()
                > PRODUCTIVITY_ARCHIVE_MAX_BYTES
        {
            return Err(archive_error());
        }
        let mut global = BTreeSet::new();
        crate::knowledge_topics::validate_folders(&self.personal_knowledge_folders)?;
        for p in &self.projects {
            crate::knowledge_topics::validate_folders(&p.knowledge_folders)?;
            valid_text(&p.id, 200)?;
            valid_text(&p.name, 200)?;
            if !global.insert(p.id.clone())
                || p.orders.len() > 1000
                || p.skills.len() > 1000
                || p.knowledge.len() > 2000
                || p.knowledge_revisions.len() > 20000
                || p.schedules.len() > 500
            {
                return Err(archive_error());
            }
            let mut orders = BTreeSet::new();
            for order in &p.orders {
                valid_text(&order.id, 200)?;
                valid_text(&order.title, 300)?;
                if order.draft.len() > 128_000 || !global.insert(order.id.clone()) {
                    return Err(archive_error());
                }
                orders.insert(order.id.clone());
                if order.messages.len() > 10_000 || order.artifacts.len() > 1000 {
                    return Err(archive_error());
                }
                let mut identifiers = BTreeSet::new();
                for message in &order.messages {
                    valid_text(&message.id, 200)?;
                    if let Some(source) = &message.source_id {
                        valid_text(source, 200)?;
                    }
                    valid_text(&message.role, 40)?;
                    if !matches!(
                        message.role.as_str(),
                        "system" | "user" | "assistant" | "tool"
                    ) || message.content.len() > 1_048_576
                        || !identifiers.insert(message.id.clone())
                    {
                        return Err(archive_error());
                    }
                }
                for artifact in &order.artifacts {
                    valid_text(&artifact.id, 200)?;
                    if let Some(source) = &artifact.source_id {
                        valid_text(source, 200)?;
                    }
                    valid_text(&artifact.name, 300)?;
                    valid_text(&artifact.media_type, 120)?;
                    if artifact.content.len() > PRODUCTIVITY_ARCHIVE_MAX_BYTES
                        || artifact.size != artifact.content.len() as u64
                        || artifact.sha256 != hash(artifact.content.as_bytes())
                        || !identifiers.insert(artifact.id.clone())
                    {
                        return Err(archive_error());
                    }
                }
            }
            for s in &p.skills {
                if s.project_id != p.id || !global.insert(s.id.clone()) {
                    return Err(archive_error());
                }
                validate_skill(&WriteSkill {
                    run_mode: Default::default(),
                    auto_learn: false,
                    id: None,
                    expected_revision: 0,
                    name: s.name.clone(),
                    description: s.description.clone(),
                    instructions: s.instructions.clone(),
                    trigger_pattern: s.trigger_pattern.clone(),
                    prompt_template: s.prompt_template.clone(),
                    enabled: s.enabled,
                })?;
            }
            for d in &p.knowledge {
                let folders = if crate::knowledge_topics::personal(d) {
                    &self.personal_knowledge_folders
                } else {
                    &p.knowledge_folders
                };
                if d.folder_id
                    .as_ref()
                    .is_some_and(|id| !folders.iter().any(|f| &f.id == id))
                {
                    return Err(archive_error());
                }
                if d.project_id != p.id || d.revision < 1 || !global.insert(d.id.clone()) {
                    return Err(archive_error());
                }
                validate_knowledge(&WriteKnowledge {
                    location: None,
                    memory: d.memory.clone(),
                    provenance: Some(d.provenance.clone()),
                    id: None,
                    expected_revision: 0,
                    kind: d.kind.clone(),
                    title: d.title.clone(),
                    content: d.content.clone(),
                    tags: d.tags.clone(),
                })?;
            }
            let mut revision_keys = BTreeSet::new();
            for revision in &p.knowledge_revisions {
                let doc: KnowledgeDocument = serde_json::from_value(revision.document.clone())
                    .map_err(|_| archive_error())?;
                if revision.revision < 1
                    || doc.id != revision.document_id
                    || doc.project_id != p.id
                    || doc.revision != revision.revision
                    || !p
                        .knowledge
                        .iter()
                        .any(|current| current.id == doc.id && current.revision >= doc.revision)
                    || hash(doc.content.as_bytes()) != revision.content_sha256
                    || doc.provenance != revision.provenance
                    || canonical_hash(&revision.provenance) != revision.provenance_sha256
                    || !revision_keys.insert((doc.id.clone(), doc.revision))
                {
                    return Err(archive_error());
                }
                validate_knowledge(&WriteKnowledge {
                    location: None,
                    memory: doc.memory.clone(),
                    provenance: Some(doc.provenance),
                    id: None,
                    expected_revision: 0,
                    kind: doc.kind,
                    title: doc.title,
                    content: doc.content,
                    tags: doc.tags,
                })?;
            }
            for s in &p.schedules {
                if s.project_id != p.id
                    || !global.insert(s.id.clone())
                    || !orders.contains(&s.template.order_id)
                {
                    return Err(archive_error());
                }
                validate_schedule(&WriteSchedule {
                    id: None,
                    expected_revision: 0,
                    name: s.name.clone(),
                    timezone: s.timezone.clone(),
                    recurrence: s.recurrence.clone(),
                    template: s.template.clone(),
                    paused: true,
                })?;
            }
        }
        Ok(())
    }
}

/// Trusted import callers publish inert history in their existing transaction.
/// This grants no run, tool, principal, folder or credential authority.
pub async fn insert_imported_history_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    history: ImportedProductHistory,
) -> Result<()> {
    let payload = json_string(&history)?;
    if history.revision != 1
        || history.messages.len() > 10_000
        || history.artifacts.len() > 1_000
        || payload.len() > PRODUCTIVITY_ARCHIVE_MAX_BYTES
    {
        return Err(archive_error());
    }
    product_project(tx, &history.project_id).await?;
    let exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM orders WHERE project_id=$1 AND id=$2")
            .bind(&history.project_id)
            .bind(&history.order_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    if exists != 1 {
        return Err(Error::NotFound);
    }
    let mut ids = BTreeSet::new();
    for message in &history.messages {
        valid_text(&message.id, 200)?;
        if let Some(source) = &message.source_id {
            valid_text(source, 200)?;
        }
        if !matches!(
            message.role.as_str(),
            "system" | "user" | "assistant" | "tool"
        ) || message.content.len() > 1_048_576
            || !ids.insert(message.id.clone())
        {
            return Err(archive_error());
        }
    }
    for artifact in &history.artifacts {
        valid_text(&artifact.id, 200)?;
        if let Some(source) = &artifact.source_id {
            valid_text(source, 200)?;
        }
        valid_text(&artifact.name, 300)?;
        valid_text(&artifact.media_type, 120)?;
        if artifact.size != artifact.content.len() as u64
            || artifact.sha256 != hash(artifact.content.as_bytes())
            || !ids.insert(artifact.id.clone())
        {
            return Err(archive_error());
        }
    }
    let changed = sqlx::query("INSERT INTO productivity_objects(id,project_id,kind,revision,payload) VALUES($1,$2,'history',1,$3) ON CONFLICT(id) DO NOTHING")
        .bind(format!("history:{}",history.order_id))
        .bind(history.project_id)
        .bind(payload)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    if changed.rows_affected() != 1 {
        return Err(Error::Conflict);
    }
    Ok(())
}

impl Store {
    pub async fn productivity_export(&self, project: Option<&str>) -> Result<ProductivityArchive> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        if !self.sqlite {
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
        }
        let rows = sqlx::query(
            "SELECT payload FROM projects WHERE $1 IS NULL OR id=$1 ORDER BY id LIMIT 101",
        )
        .bind(project)
        .fetch_all(&mut *tx)
        .await
        .map_err(storage_error)?;
        if project.is_some() && rows.is_empty() {
            return Err(Error::NotFound);
        }
        if rows.len() > 100 {
            return Err(archive_error());
        }
        let mut archive = ProductivityArchive {
            personal_knowledge_folders: crate::knowledge_topics::folders_in_tx(&mut tx, "", true)
                .await?
                .folders,
            format: "localai-cowork-product".into(),
            version: 1,
            created_at: now(),
            projects: Vec::new(),
        };
        for row in rows {
            let p: Project = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let mut out = ArchivedProductProject {
                knowledge_folders: crate::knowledge_topics::folders_in_tx(&mut tx, &p.id, false)
                    .await?
                    .folders,
                id: p.id.clone(),
                name: p.name,
                revision: p.revision,
                orders: Vec::new(),
                skills: Vec::new(),
                knowledge: Vec::new(),
                knowledge_revisions: Vec::new(),
                schedules: Vec::new(),
            };
            for row in
                sqlx::query("SELECT payload FROM orders WHERE project_id=$1 ORDER BY id LIMIT 1001")
                    .bind(&p.id)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(storage_error)?
            {
                let o: Order = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                out.orders.push(ArchivedProductOrder {
                    id: o.id,
                    title: o.title,
                    draft: o.draft,
                    revision: o.revision,
                    messages: Vec::new(),
                    artifacts: Vec::new(),
                });
                let order = out.orders.last_mut().ok_or(Error::Conflict)?;
                for row in sqlx::query(
                    "SELECT payload FROM messages WHERE order_id=$1 ORDER BY position LIMIT 10001",
                )
                .bind(&order.id)
                .fetch_all(&mut *tx)
                .await
                .map_err(storage_error)?
                {
                    let m: Message =
                        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                    order.messages.push(ArchivedProductMessage {
                        id: m.id,
                        source_id: None,
                        role: m.role,
                        content: m.content,
                        created_at: m.created_at,
                    });
                }
                for row in sqlx::query(
                    "SELECT payload FROM artifacts WHERE order_id=$1 ORDER BY id LIMIT 1001",
                )
                .bind(&order.id)
                .fetch_all(&mut *tx)
                .await
                .map_err(storage_error)?
                {
                    let a: Artifact =
                        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                    order.artifacts.push(ArchivedProductArtifact {
                        id: a.id,
                        source_id: None,
                        name: a.name,
                        media_type: a.media_type,
                        size: a.size,
                        sha256: a.sha256,
                        content: a.content,
                        created_at: a.created_at,
                    });
                }
                if let Some(row)=sqlx::query("SELECT payload FROM productivity_objects WHERE project_id=$1 AND id=$2 AND kind='history'").bind(&p.id).bind(format!("history:{}",order.id)).fetch_optional(&mut *tx).await.map_err(storage_error)?{let history:ImportedProductHistory=parse(&row.try_get::<String,_>("payload").map_err(storage_error)?)?;order.messages.extend(history.messages);order.artifacts.extend(history.artifacts);}
            }
            for row in sqlx::query("SELECT kind,payload FROM productivity_objects WHERE project_id=$1 ORDER BY id LIMIT 4001").bind(&p.id).fetch_all(&mut *tx).await.map_err(storage_error)? {let body:String=row.try_get("payload").map_err(storage_error)?;match row.try_get::<String,_>("kind").map_err(storage_error)?.as_str(){"skill"=>out.skills.push(parse(&body)?),"knowledge"=>out.knowledge.push(parse(&body)?),"history"=>{},_=>return Err(archive_error())}}
            for row in sqlx::query("SELECT payload FROM productivity_schedules WHERE project_id=$1 ORDER BY id LIMIT 501").bind(&p.id).fetch_all(&mut *tx).await.map_err(storage_error)? {let mut s:ProductSchedule=parse(&row.try_get::<String,_>("payload").map_err(storage_error)?)?;s.active_occurrence=None;s.last_run_id=None;s.last_error=None;s.paused=true;out.schedules.push(s);}
            for document in &out.knowledge {
                let revisions = crate::knowledge_provenance::all_revisions(
                    &mut tx,
                    &crate::knowledge_provenance::scope(&p.id, None),
                    &document.id,
                )
                .await?;
                if out.knowledge_revisions.len() + revisions.len() > 20000 {
                    return Err(archive_error());
                }
                out.knowledge_revisions.extend(revisions);
            }
            archive.projects.push(out);
        }
        archive.validate()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(archive)
    }
    /// Entire import and its exact replay receipt commit together. No run can be imported.
    pub async fn productivity_import(
        &self,
        input: ProductivityImportRequest,
    ) -> Result<ProductivityImportResult> {
        valid_text(&input.idempotency_key, 200)?;
        input.archive.validate()?;
        let fingerprint = canonical_hash(&input);
        let mut tx = begin_productivity(self).await?;
        if let Some(row) = sqlx::query(
            "SELECT request_hash,payload FROM productivity_imports WHERE idempotency_key=$1",
        )
        .bind(&input.idempotency_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        {
            if row
                .try_get::<String, _>("request_hash")
                .map_err(storage_error)?
                != fingerprint
            {
                return Err(Error::Conflict);
            }
            return parse(&row.try_get::<String, _>("payload").map_err(storage_error)?);
        }
        let mut result = ProductivityImportResult {
            version: 1,
            projects: Vec::new(),
            schedules_paused: 0,
        };
        let personal_folder_ids = crate::knowledge_topics::import_folders(
            &mut tx,
            "",
            true,
            &input.archive.personal_knowledge_folders,
        )
        .await?;
        for source in input.archive.projects {
            let stamp = now();
            let oldrow = sqlx::query("SELECT payload FROM projects WHERE id=$1")
                .bind(&source.id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
            let old: Option<Project> = oldrow
                .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
                .transpose()?;
            let project_id = if input.policy == ProductivityImportPolicy::Copy {
                id()
            } else {
                source.id.clone()
            };
            if old.is_some() && input.policy == ProductivityImportPolicy::Reject {
                return Err(Error::Conflict);
            }
            let replacing = old.is_some() && input.policy == ProductivityImportPolicy::Replace;
            let mut previous_revisions: BTreeMap<String, i64> = BTreeMap::new();
            if replacing {
                let p = old.as_ref().ok_or(Error::Conflict)?;
                if input.expected_project_revisions.get(&p.id) != Some(&p.revision) {
                    return Err(Error::Conflict);
                }
                let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM productivity_schedules WHERE project_id=$1 AND payload LIKE '%\"active_occurrence\":{%'").bind(&p.id).fetch_one(&mut *tx).await.map_err(storage_error)?;
                if active != 0 {
                    return Err(Error::Conflict);
                }
                // Existing live execution is not replaced or silently detached.
                let busy:i64=sqlx::query_scalar("SELECT COUNT(*) FROM runs JOIN orders ON orders.id=runs.order_id WHERE orders.project_id=$1 AND runs.state NOT IN ('completed','failed','canceled','expired')").bind(&p.id).fetch_one(&mut *tx).await.map_err(storage_error)?;
                if busy != 0 {
                    return Err(Error::Conflict);
                }
                for row in sqlx::query(
                    "SELECT id,kind,revision FROM productivity_objects WHERE project_id=$1",
                )
                .bind(&p.id)
                .fetch_all(&mut *tx)
                .await
                .map_err(storage_error)?
                {
                    let object: String = row.try_get("id").map_err(storage_error)?;
                    previous_revisions.insert(
                        object.clone(),
                        row.try_get("revision").map_err(storage_error)?,
                    );
                    product_change(
                        &mut tx,
                        &p.id,
                        &row.try_get::<String, _>("kind").map_err(storage_error)?,
                        &object,
                        true,
                        Value::Null,
                    )
                    .await?;
                }
                for row in sqlx::query(
                    "SELECT id,revision FROM productivity_schedules WHERE project_id=$1",
                )
                .bind(&p.id)
                .fetch_all(&mut *tx)
                .await
                .map_err(storage_error)?
                {
                    let object: String = row.try_get("id").map_err(storage_error)?;
                    previous_revisions.insert(
                        object.clone(),
                        row.try_get("revision").map_err(storage_error)?,
                    );
                    product_change(&mut tx, &p.id, "schedule", &object, true, Value::Null).await?;
                }
                sqlx::query("DELETE FROM productivity_objects WHERE project_id=$1")
                    .bind(&p.id)
                    .execute(&mut *tx)
                    .await
                    .map_err(storage_error)?;
                sqlx::query("DELETE FROM productivity_schedules WHERE project_id=$1")
                    .bind(&p.id)
                    .execute(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            }
            let project = Project {
                id: project_id.clone(),
                name: source.name,
                revision: if replacing {
                    product_project(&mut tx, &project_id)
                        .await?
                        .revision
                        .checked_add(1)
                        .ok_or(Error::Conflict)?
                } else {
                    1
                },
                created_at: if replacing {
                    old.as_ref().ok_or(Error::Conflict)?.created_at
                } else {
                    stamp
                },
            };
            if replacing {
                sqlx::query("UPDATE projects SET payload=$1 WHERE id=$2")
                    .bind(json_string(&project)?)
                    .bind(&project.id)
                    .execute(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            } else {
                sqlx::query("INSERT INTO projects(id,payload) VALUES($1,$2)")
                    .bind(&project.id)
                    .bind(json_string(&project)?)
                    .execute(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            }
            product_change(
                &mut tx,
                &project.id,
                "project",
                &project.id,
                false,
                serde_json::to_value(&project).map_err(storage_error)?,
            )
            .await?;
            let folder_ids = crate::knowledge_topics::import_folders(
                &mut tx,
                &project.id,
                false,
                &source.knowledge_folders,
            )
            .await?;
            let mut order_ids = BTreeMap::new();
            for o in source.orders {
                let order_id = if input.policy == ProductivityImportPolicy::Copy {
                    id()
                } else {
                    o.id.clone()
                };
                let oldrow = sqlx::query("SELECT payload FROM orders WHERE id=$1")
                    .bind(&order_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(storage_error)?;
                let previous: Option<Order> = oldrow
                    .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
                    .transpose()?;
                if let Some(p) = &previous
                    && (!replacing
                        || p.project_id != project.id
                        || input.expected_order_revisions.get(&p.id) != Some(&p.revision))
                {
                    return Err(Error::Conflict);
                }
                let order = Order {
                    id: order_id.clone(),
                    project_id: project.id.clone(),
                    title: o.title,
                    draft: o.draft,
                    revision: previous.as_ref().map_or(1, |p| p.revision + 1),
                    created_at: previous.as_ref().map_or(stamp, |p| p.created_at),
                };
                if let Some(previous) = &previous {
                    let affected = sqlx::query(
                        "UPDATE orders SET revision=$1,payload=$2 WHERE id=$3 AND revision=$4",
                    )
                    .bind(order.revision)
                    .bind(json_string(&order)?)
                    .bind(&order.id)
                    .bind(previous.revision)
                    .execute(&mut *tx)
                    .await
                    .map_err(storage_error)?
                    .rows_affected();
                    if affected != 1 {
                        return Err(Error::Conflict);
                    }
                } else {
                    sqlx::query(
                        "INSERT INTO orders(id,project_id,revision,payload) VALUES($1,$2,$3,$4)",
                    )
                    .bind(&order.id)
                    .bind(&project.id)
                    .bind(order.revision)
                    .bind(json_string(&order)?)
                    .execute(&mut *tx)
                    .await
                    .map_err(storage_error)?;
                }
                product_change(
                    &mut tx,
                    &project.id,
                    "order",
                    &order.id,
                    false,
                    serde_json::to_value(&order).map_err(storage_error)?,
                )
                .await?;
                if !o.messages.is_empty() || !o.artifacts.is_empty() {
                    let history_id = format!("history:{}", order.id);
                    let revision = previous_revisions
                        .get(&history_id)
                        .copied()
                        .unwrap_or(0)
                        .checked_add(1)
                        .ok_or(Error::Conflict)?;
                    let history = ImportedProductHistory {
                        project_id: project.id.clone(),
                        order_id: order.id.clone(),
                        messages: o.messages,
                        artifacts: o.artifacts,
                        revision,
                        imported_at: stamp,
                    };
                    save_object(
                        &mut tx,
                        &project.id,
                        &history_id,
                        "history",
                        revision,
                        &history,
                    )
                    .await?;
                }
                order_ids.insert(o.id, order_id);
            }
            for mut s in source.skills {
                s.id = if input.policy == ProductivityImportPolicy::Copy {
                    id()
                } else {
                    s.id
                };
                s.project_id = project.id.clone();
                s.revision = previous_revisions
                    .get(&s.id)
                    .copied()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or(Error::Conflict)?;
                s.created_at = stamp;
                s.updated_at = stamp;
                save_object(&mut tx, &project.id, &s.id, "skill", s.revision, &s).await?;
            }
            for mut d in source.knowledge {
                let source_folders = if crate::knowledge_topics::personal(&d) {
                    &personal_folder_ids
                } else {
                    &folder_ids
                };
                d.folder_id = d
                    .folder_id
                    .as_ref()
                    .and_then(|f| source_folders.get(f))
                    .cloned();
                let source_id = d.id.clone();
                let original = d.clone();
                d.id = if input.policy == ProductivityImportPolicy::Copy {
                    id()
                } else {
                    d.id
                };
                d.project_id = project.id.clone();
                if let Some(metadata) = &mut d.memory {
                    metadata.enabled = false;
                }
                let previous_revision = previous_revisions.get(&d.id).copied().unwrap_or(0);
                for historical in source
                    .knowledge_revisions
                    .iter()
                    .filter(|r| r.document_id == source_id)
                {
                    let mut imported: KnowledgeDocument =
                        serde_json::from_value(historical.document.clone())
                            .map_err(|_| archive_error())?;
                    let original_revision = imported.revision;
                    imported.provenance =
                        crate::knowledge_provenance::archive_source(&imported, &source.id);
                    imported.id = d.id.clone();
                    imported.project_id = project.id.clone();
                    imported.revision = previous_revision
                        .checked_add(original_revision)
                        .ok_or(Error::Conflict)?;
                    crate::knowledge_provenance::record(
                        &mut tx,
                        &crate::knowledge_provenance::scope(&project.id, None),
                        &imported.id,
                        imported.revision,
                        &imported.content,
                        &imported.provenance,
                        &imported,
                    )
                    .await?;
                }
                d.provenance = crate::knowledge_provenance::archive_source(&original, &source.id);
                d.revision = previous_revision
                    .checked_add(original.revision)
                    .ok_or(Error::Conflict)?
                    .checked_add(1)
                    .ok_or(Error::Conflict)?;
                d.created_at = stamp;
                d.updated_at = stamp;
                save_object(&mut tx, &project.id, &d.id, "knowledge", d.revision, &d).await?;
                crate::knowledge_provenance::record(
                    &mut tx,
                    &crate::knowledge_provenance::scope(&project.id, None),
                    &d.id,
                    d.revision,
                    &d.content,
                    &d.provenance,
                    &d,
                )
                .await?;
            }
            for mut s in source.schedules {
                s.id = if input.policy == ProductivityImportPolicy::Copy {
                    id()
                } else {
                    s.id
                };
                s.project_id = project.id.clone();
                s.template.order_id = order_ids
                    .get(&s.template.order_id)
                    .ok_or_else(archive_error)?
                    .clone();
                s.active_occurrence = None;
                s.last_run_id = None;
                s.last_error = None;
                s.paused = true;
                s.next_run = productivity_next_run(&s.recurrence, &s.timezone, stamp)?;
                s.revision = previous_revisions
                    .get(&s.id)
                    .copied()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or(Error::Conflict)?;
                s.created_at = stamp;
                s.updated_at = stamp;
                save_schedule(&mut tx, &s).await?;
                result.schedules_paused += 1;
            }
            result.projects.push(ProductivityImportedProject {
                source_id: source.id,
                project: product_project(&mut tx, &project.id).await?,
                order_ids,
            });
        }
        sqlx::query("INSERT INTO productivity_imports(idempotency_key,request_hash,payload) VALUES($1,$2,$3)").bind(&input.idempotency_key).bind(fingerprint).bind(json_string(&result)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn productivity_imported_history(
        &self,
        project: &str,
        order: &str,
    ) -> Result<Option<ImportedProductHistory>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        product_project(&mut tx, project).await?;
        let exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM orders WHERE project_id=$1 AND id=$2")
                .bind(project)
                .bind(order)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if exists != 1 {
            return Err(Error::NotFound);
        }
        sqlx::query("SELECT payload FROM productivity_objects WHERE project_id=$1 AND id=$2 AND kind='history'").bind(project).bind(format!("history:{order}")).fetch_optional(&mut *tx).await.map_err(storage_error)?.map(|r|parse(&r.try_get::<String,_>("payload").map_err(storage_error)?)).transpose()
    }
}
