//! Personal-owner product data. Every identifier is checked against its actual project.
use super::*;
#[path = "productivity_cron.rs"]
mod native_cron;
use chrono::{Datelike, LocalResult, TimeZone};
use chrono_tz::Tz;
use native_cron::native_cron_next;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub use cowork_contracts::{KnowledgeDocument, KnowledgeKind, WriteKnowledge};
pub use cowork_contracts::{Skill, WriteSkill};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionRequest {
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeHit {
    pub document: KnowledgeDocument,
    pub score: u32,
}

pub use cowork_contracts::ScheduleRecurrence;
pub use cowork_workflow_contracts::ScheduleOccurrence;
pub use cowork_workflow_contracts::{ProductSchedule, ScheduledRunTemplate, WriteSchedule};
pub(super) fn occurrence_hash(active: &ScheduleOccurrence) -> String {
    active.workflow_request.as_ref().map_or_else(
        || canonical_hash(&active.request),
        crate::workflows::product::request_hash,
    )
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductivityChange {
    pub sequence: i64,
    pub project_id: String,
    pub kind: String,
    pub object_id: String,
    pub deleted: bool,
    pub payload: Value,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductivitySyncPage {
    pub format_version: u32,
    pub changes: Vec<ProductivityChange>,
    pub next_cursor: i64,
    pub has_more: bool,
}

pub(super) async fn begin_productivity(store: &Store) -> Result<sqlx::Transaction<'_, sqlx::Any>> {
    let mut tx = store.pool.begin().await.map_err(storage_error)?;
    // A consistent lock order for imports, mutations, claims and event cursors.
    sqlx::query("UPDATE productivity_guard SET id=id WHERE id=1")
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
    Ok(tx)
}
pub(super) async fn product_project(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
) -> Result<Project> {
    let row = sqlx::query("SELECT payload FROM projects WHERE id=$1 AND id NOT IN (SELECT project_id FROM native_project_retirements)")
        .bind(project)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)
}
pub(super) async fn product_change(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    kind: &str,
    object: &str,
    deleted: bool,
    payload: Value,
) -> Result<()> {
    let mut record = product_project(tx, project).await?;
    record.revision = record.revision.checked_add(1).ok_or(Error::Conflict)?;
    sqlx::query("UPDATE projects SET payload=$1 WHERE id=$2")
        .bind(json_string(&record)?)
        .bind(project)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    let seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(sequence),0)+1 FROM productivity_changes WHERE project_id=$1",
    )
    .bind(project)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    sqlx::query("INSERT INTO productivity_changes(project_id,sequence,kind,object_id,deleted,payload,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(project).bind(seq).bind(kind).bind(object).bind(i64::from(deleted)).bind(json_string(&payload)?).bind(now())
        .execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
async fn existing_object<T: serde::de::DeserializeOwned>(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    object: &str,
    kind: &str,
) -> Result<T> {
    let row = sqlx::query(
        "SELECT payload FROM productivity_objects WHERE project_id=$1 AND id=$2 AND kind=$3",
    )
    .bind(project)
    .bind(object)
    .bind(kind)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .ok_or(Error::NotFound)?;
    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)
}
pub(super) async fn save_object(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    object: &str,
    kind: &str,
    revision: i64,
    payload: &impl Serialize,
) -> Result<()> {
    let body = json_string(payload)?;
    let result=sqlx::query("INSERT INTO productivity_objects(id,project_id,kind,revision,payload) VALUES($1,$2,$3,$4,$5) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload WHERE productivity_objects.project_id=excluded.project_id AND productivity_objects.kind=excluded.kind")
        .bind(object).bind(project).bind(kind).bind(revision).bind(&body).execute(&mut **tx).await.map_err(storage_error)?;
    if result.rows_affected() != 1 {
        return Err(Error::Conflict);
    }
    product_change(tx, project, kind, object, false, parse(&body)?).await
}
pub(super) fn validate_skill(input: &WriteSkill) -> Result<()> {
    valid_text(&input.name, 200)?;
    valid_text(&input.instructions, 64_000)?;
    if input.description.len() > 4000 || input.description.contains('\0') {
        return Err(Error::Validation("Invalid skill description".into()));
    }
    if let Some(pattern) = &input.trigger_pattern {
        valid_text(pattern, 200)?;
        if pattern.chars().any(char::is_control) {
            return Err(Error::Validation("invalid_skill_trigger".into()));
        }
    }
    if let Some(template) = &input.prompt_template {
        valid_text(template, 64_000)?;
    }
    Ok(())
}
pub(super) fn validate_knowledge(input: &WriteKnowledge) -> Result<()> {
    if let Some(metadata) = &input.memory {
        if input.kind != KnowledgeKind::Memory {
            return Err(Error::Validation(
                "Memory metadata requires a memory document".into(),
            ));
        }
        crate::memory_context::validate(metadata)?;
    }
    if let Some(provenance) = &input.provenance {
        cowork_domain::knowledge_provenance::validate(provenance)?;
    }
    valid_text(&input.title, 300)?;
    valid_text(&input.content, 256_000)?;
    if input.tags.len() > 32 {
        return Err(Error::Validation("Too many knowledge tags".into()));
    }
    let mut tags = BTreeSet::new();
    for tag in &input.tags {
        valid_text(tag, 80)?;
        if !tags.insert(tag.to_lowercase()) {
            return Err(Error::Validation("Duplicate knowledge tag".into()));
        }
    }
    Ok(())
}
pub fn productivity_next_run(
    recurrence: &ScheduleRecurrence,
    timezone: &str,
    after: i64,
) -> Result<Option<i64>> {
    let tz: Tz = timezone
        .parse()
        .map_err(|_| Error::Validation("Unknown IANA timezone".into()))?;
    let utc = chrono::Utc
        .timestamp_millis_opt(after)
        .single()
        .ok_or_else(|| Error::Validation("Invalid schedule date".into()))?;
    let (hour, minute, weekday) = match recurrence {
        ScheduleRecurrence::Cron { expression } => {
            return native_cron_next(expression, &tz, &utc);
        }
        ScheduleRecurrence::Once { at_ms } => {
            if chrono::Utc.timestamp_millis_opt(*at_ms).single().is_none() {
                return Err(Error::Validation("Invalid schedule date".into()));
            }
            return Ok((*at_ms > after).then_some(*at_ms));
        }
        ScheduleRecurrence::Interval { seconds } => {
            if !(60..=31_536_000).contains(seconds) {
                return Err(Error::Validation(
                    "Schedule interval must be 60–31536000 seconds".into(),
                ));
            }
            return after
                .checked_add(i64::from(*seconds) * 1000)
                .map(Some)
                .ok_or_else(|| Error::Validation("Schedule date overflow".into()));
        }
        ScheduleRecurrence::Daily { hour, minute } => (*hour, *minute, None),
        ScheduleRecurrence::Weekly {
            weekday,
            hour,
            minute,
        } => {
            if !(1..=7).contains(weekday) {
                return Err(Error::Validation("Schedule weekday must be 1–7".into()));
            }
            (*hour, *minute, Some(*weekday))
        }
    };
    if hour > 23 || minute > 59 {
        return Err(Error::Validation("Invalid schedule wall time".into()));
    }
    let local = utc.with_timezone(&tz);
    for day in 0..370 {
        let date = local
            .date_naive()
            .checked_add_days(chrono::Days::new(day))
            .ok_or(Error::Conflict)?;
        if weekday.is_some_and(|w| date.weekday().number_from_monday() != w) {
            continue;
        }
        let naive = date.and_hms_opt(hour, minute, 0).ok_or(Error::Conflict)?;
        let instant = match tz.from_local_datetime(&naive) {
            LocalResult::Single(v) => v,
            LocalResult::Ambiguous(a, b) => a.min(b),
            LocalResult::None => continue,
        };
        if instant.timestamp_millis() > after {
            return Ok(Some(instant.timestamp_millis()));
        }
    }
    Err(Error::Validation(
        "No schedule occurrence in the next year".into(),
    ))
}
pub(super) fn validate_schedule(input: &WriteSchedule) -> Result<()> {
    valid_text(&input.name, 200)?;
    valid_text(&input.template.prompt, 64_000)?;
    valid_text(&input.template.provider_id, 200)?;
    if input.template.provider_revision < 1 {
        return Err(Error::Validation("Invalid provider revision".into()));
    }
    cowork_domain::validate_budgets(&input.template.budgets)?;
    if let Some(config) = &input.template.agent_config {
        cowork_domain::validate_agent_values(config)?;
    }
    if let Some(config) = input
        .template
        .workflow
        .as_ref()
        .and_then(|request| request.agent_config.as_ref())
    {
        cowork_domain::validate_agent_values(config)?;
    }
    productivity_next_run(&input.recurrence, &input.timezone, now())?;
    Ok(())
}
pub(super) async fn save_schedule(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    schedule: &ProductSchedule,
) -> Result<()> {
    let result=sqlx::query("INSERT INTO productivity_schedules(id,project_id,revision,next_run,paused,payload) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,next_run=excluded.next_run,paused=excluded.paused,payload=excluded.payload WHERE productivity_schedules.project_id=excluded.project_id")
        .bind(&schedule.id).bind(&schedule.project_id).bind(schedule.revision).bind(if schedule.active_occurrence.is_some(){i64::MAX}else{schedule.next_run.unwrap_or(i64::MAX)}).bind(i64::from(schedule.paused)).bind(json_string(schedule)?)
        .execute(&mut **tx).await.map_err(storage_error)?;
    if result.rows_affected() != 1 {
        return Err(Error::Conflict);
    }
    product_change(
        tx,
        &schedule.project_id,
        "schedule",
        &schedule.id,
        false,
        serde_json::to_value(schedule).map_err(storage_error)?,
    )
    .await
}
pub(super) async fn read_schedule(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    schedule: &str,
) -> Result<ProductSchedule> {
    let row =
        sqlx::query("SELECT payload FROM productivity_schedules WHERE project_id=$1 AND id=$2")
            .bind(project)
            .bind(schedule)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)
}

pub(super) async fn write_skill_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    input: WriteSkill,
) -> Result<Skill> {
    validate_skill(&input)?;
    product_project(tx, project).await?;
    let old: Option<Skill> = match &input.id {
        Some(key) => Some(existing_object(tx, project, key, "skill").await?),
        None => None,
    };
    if old.as_ref().map_or(0, |v| v.revision) != input.expected_revision {
        return Err(Error::Conflict);
    }
    let stamp = now();
    let skill = Skill {
        run_mode: input.run_mode,
        auto_learn: input.auto_learn,
        id: old.as_ref().map_or_else(id, |v| v.id.clone()),
        project_id: project.into(),
        name: input.name.trim().into(),
        description: input.description,
        instructions: input.instructions,
        trigger_pattern: input.trigger_pattern,
        prompt_template: input.prompt_template,
        enabled: input.enabled,
        revision: input
            .expected_revision
            .checked_add(1)
            .ok_or(Error::Conflict)?,
        created_at: old.as_ref().map_or(stamp, |v| v.created_at),
        updated_at: stamp,
    };
    if old.is_none() {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM productivity_objects WHERE project_id=$1 AND kind='skill'",
        )
        .bind(project)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
        if count >= 1000 {
            return Err(Error::Validation("Project skill limit exceeded".into()));
        }
    }
    save_object(tx, project, &skill.id, "skill", skill.revision, &skill).await?;
    Ok(skill)
}

pub(super) async fn write_knowledge_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    input: WriteKnowledge,
) -> Result<KnowledgeDocument> {
    validate_knowledge(&input)?;
    product_project(tx, project).await?;
    let old: Option<KnowledgeDocument> = match &input.id {
        Some(key) => Some(existing_object(tx, project, key, "knowledge").await?),
        None => None,
    };
    if old.as_ref().map_or(0, |v| v.revision) != input.expected_revision {
        return Err(Error::Conflict);
    }
    let memory = if input.kind == KnowledgeKind::Memory {
        input
            .memory
            .clone()
            .or_else(|| old.as_ref().and_then(|v| v.memory.clone()))
    } else {
        None
    };
    if let Some(metadata) = &memory {
        crate::memory_context::bind(tx, project, metadata).await?;
    }
    let folder_id = input.location.as_ref().map_or_else(
        || old.as_ref().and_then(|d| d.folder_id.clone()),
        |location| location.folder_id.clone(),
    );
    crate::knowledge_topics::bind_location(
        tx,
        project,
        input.kind == KnowledgeKind::Memory
            && memory
                .as_ref()
                .is_some_and(|m| m.scope == MemoryScope::User),
        folder_id.as_deref(),
    )
    .await?;
    let mut provenance = input.provenance.unwrap_or_else(|| {
        old.as_ref()
            .map_or_else(KnowledgeProvenance::user_authored, |v| v.provenance.clone())
    });
    crate::knowledge_provenance::bind_sources(tx, project, None, &mut provenance).await?;
    if let Some(previous) = &old {
        crate::knowledge_provenance::record(
            tx,
            &crate::knowledge_provenance::scope(project, None),
            &previous.id,
            previous.revision,
            &previous.content,
            &previous.provenance,
            previous,
        )
        .await?;
    }
    let stamp = now();
    let doc = KnowledgeDocument {
        folder_id,
        memory,
        provenance,
        id: old.as_ref().map_or_else(id, |v| v.id.clone()),
        project_id: project.into(),
        kind: input.kind,
        title: input.title.trim().into(),
        content: input.content,
        tags: input.tags,
        revision: input
            .expected_revision
            .checked_add(1)
            .ok_or(Error::Conflict)?,
        created_at: old.as_ref().map_or(stamp, |v| v.created_at),
        updated_at: stamp,
    };
    if old.is_none() {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM productivity_objects WHERE project_id=$1 AND kind='knowledge'",
        )
        .bind(project)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
        if count >= 2000 {
            return Err(Error::Validation("Project knowledge limit exceeded".into()));
        }
    }
    save_object(tx, project, &doc.id, "knowledge", doc.revision, &doc).await?;
    crate::memory_context::index(tx, &doc).await?;
    crate::knowledge_provenance::record(
        tx,
        &crate::knowledge_provenance::scope(project, None),
        &doc.id,
        doc.revision,
        &doc.content,
        &doc.provenance,
        &doc,
    )
    .await?;
    Ok(doc)
}

pub(super) async fn delete_object_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    object: &str,
    kind: &str,
    revision: i64,
) -> Result<()> {
    if !matches!(kind, "skill" | "knowledge") {
        return Err(Error::Validation("Invalid product object kind".into()));
    }
    product_project(tx, project).await?;
    let result=sqlx::query("DELETE FROM productivity_objects WHERE project_id=$1 AND id=$2 AND kind=$3 AND revision=$4").bind(project).bind(object).bind(kind).bind(revision).execute(&mut **tx).await.map_err(storage_error)?;
    if result.rows_affected() != 1 {
        return Err(Error::Conflict);
    }
    product_change(tx, project, kind, object, true, Value::Null).await?;
    Ok(())
}

impl Store {
    pub async fn productivity_skills(&self, project: &str) -> Result<Vec<Skill>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        product_project(&mut tx, project).await?;
        let rows=sqlx::query("SELECT payload FROM productivity_objects WHERE project_id=$1 AND kind='skill' ORDER BY id LIMIT 1001")
            .bind(project).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 1000 {
            return Err(Error::Validation("Project skill limit exceeded".into()));
        }
        rows.into_iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect()
    }
    pub async fn productivity_write_skill(
        &self,
        project: &str,
        input: WriteSkill,
    ) -> Result<Skill> {
        let mut tx = begin_productivity(self).await?;
        let result = write_skill_in_tx(&mut tx, project, input).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn productivity_write_knowledge(
        &self,
        project: &str,
        input: WriteKnowledge,
    ) -> Result<KnowledgeDocument> {
        let mut tx = begin_productivity(self).await?;
        let result = write_knowledge_in_tx(&mut tx, project, input).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn productivity_knowledge(
        &self,
        project: &str,
        query: &str,
        kind: Option<KnowledgeKind>,
        limit: usize,
    ) -> Result<Vec<KnowledgeHit>> {
        if query.len() > 500 || limit == 0 || limit > 2000 {
            return Err(Error::Validation("Invalid knowledge search".into()));
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        product_project(&mut tx, project).await?;
        let terms: Vec<String> = query
            .split_whitespace()
            .map(str::to_lowercase)
            .take(16)
            .collect();
        let rows=sqlx::query("SELECT payload FROM productivity_objects WHERE kind='knowledge' AND (project_id=$1 OR id IN(SELECT document_id FROM native_user_memories WHERE project_id NOT IN(SELECT project_id FROM native_project_retirements))) ORDER BY id LIMIT 2001").bind(project).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 2000 {
            return Err(Error::Validation("Project knowledge limit exceeded".into()));
        }
        let mut hits = Vec::new();
        for row in rows {
            let doc: KnowledgeDocument =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            if kind.as_ref().is_some_and(|k| k != &doc.kind) {
                continue;
            }
            let title = doc.title.to_lowercase();
            let content = doc.content.to_lowercase();
            let tags = doc.tags.join(" ").to_lowercase();
            let memory = doc
                .memory
                .as_ref()
                .map(|m| {
                    format!(
                        "{} {}",
                        m.category,
                        m.profile_key.as_deref().unwrap_or_default()
                    )
                })
                .unwrap_or_default()
                .to_lowercase();
            if !terms.iter().all(|t| {
                title.contains(t) || content.contains(t) || tags.contains(t) || memory.contains(t)
            }) {
                continue;
            }
            let score = terms
                .iter()
                .map(|t| {
                    u32::from(title.contains(t)) * 5
                        + u32::from(tags.contains(t)) * 3
                        + u32::from(content.contains(t))
                        + u32::from(memory.contains(t)) * 2
                })
                .sum();
            hits.push(KnowledgeHit {
                document: doc,
                score,
            });
        }
        hits.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| b.document.updated_at.cmp(&a.document.updated_at))
                .then_with(|| a.document.id.cmp(&b.document.id))
        });
        hits.truncate(limit);
        Ok(hits)
    }
    pub async fn productivity_delete_object(
        &self,
        project: &str,
        object: &str,
        kind: &str,
        revision: i64,
    ) -> Result<()> {
        let mut tx = begin_productivity(self).await?;
        delete_object_in_tx(&mut tx, project, object, kind, revision).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn productivity_schedules(&self, project: &str) -> Result<Vec<ProductSchedule>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        product_project(&mut tx, project).await?;
        sqlx::query("SELECT payload FROM productivity_schedules WHERE project_id=$1 ORDER BY next_run,id LIMIT 501").bind(project).fetch_all(&mut *tx).await.map_err(storage_error)?.into_iter().map(|r|parse(&r.try_get::<String,_>("payload").map_err(storage_error)?)).collect()
    }
    pub async fn productivity_write_schedule(
        &self,
        project: &str,
        input: WriteSchedule,
    ) -> Result<ProductSchedule> {
        self.productivity_write_schedule_original(project, input, None)
            .await
    }
    /// Optional original key for native editors; legacy callers retain their CAS behavior.
    pub async fn productivity_write_schedule_idempotent(
        &self,
        project: &str,
        input: WriteSchedule,
        key: &str,
    ) -> Result<ProductSchedule> {
        valid_text(key, 128)?;
        self.productivity_write_schedule_original(project, input, Some(key))
            .await
    }
    async fn productivity_write_schedule_original(
        &self,
        project: &str,
        input: WriteSchedule,
        original_key: Option<&str>,
    ) -> Result<ProductSchedule> {
        validate_schedule(&input)?;
        let mut tx = begin_productivity(self).await?;
        product_project(&mut tx, project).await?;
        let scope = format!("native-schedule:{project}");
        let digest = canonical_hash(&input);
        if let Some(key) = original_key {
            let row=sqlx::query("SELECT request_sha256,receipt FROM workspace_management_operations WHERE scope=$1 AND request_key=$2").bind(&scope).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)?;
            if let Some(row) = row {
                if row
                    .try_get::<String, _>("request_sha256")
                    .map_err(storage_error)?
                    != digest
                {
                    return Err(Error::Conflict);
                }
                return parse(&row.try_get::<String, _>("receipt").map_err(storage_error)?);
            }
        }
        crate::workspace_management::order_lock(&mut tx, &input.template.order_id).await?;
        let order_row = sqlx::query("SELECT payload FROM orders WHERE project_id=$1 AND id=$2")
            .bind(project)
            .bind(&input.template.order_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        let _: Order = parse(
            &order_row
                .try_get::<String, _>("payload")
                .map_err(storage_error)?,
        )?;
        let old = match &input.id {
            Some(key) => Some(read_schedule(&mut tx, project, key).await?),
            None => None,
        };
        if old.as_ref().map_or(0, |v| v.revision) != input.expected_revision
            || old.as_ref().is_some_and(|v| v.active_occurrence.is_some())
        {
            return Err(Error::Conflict);
        }
        if old.is_none() {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM productivity_schedules WHERE project_id=$1",
            )
            .bind(project)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
            if count >= 500 {
                return Err(Error::Validation("Project schedule limit exceeded".into()));
            }
        }
        let stamp = now();
        let next = productivity_next_run(&input.recurrence, &input.timezone, stamp)?;
        if next.is_none() && !input.paused {
            return Err(Error::Validation(
                "One-time schedule must be in the future".into(),
            ));
        }
        let s = ProductSchedule {
            id: old.as_ref().map_or_else(id, |v| v.id.clone()),
            project_id: project.into(),
            name: input.name.trim().into(),
            timezone: input.timezone,
            recurrence: input.recurrence,
            template: input.template,
            paused: input.paused,
            next_run: next,
            active_occurrence: None,
            last_run_id: old.as_ref().and_then(|v| v.last_run_id.clone()),
            last_error: None,
            revision: input
                .expected_revision
                .checked_add(1)
                .ok_or(Error::Conflict)?,
            created_at: old.as_ref().map_or(stamp, |v| v.created_at),
            updated_at: stamp,
        };
        save_schedule(&mut tx, &s).await?;
        if let Some(key) = original_key {
            sqlx::query("INSERT INTO workspace_management_operations(scope,request_key,request_sha256,receipt) VALUES($1,$2,$3,$4)").bind(&scope).bind(key).bind(digest).bind(json_string(&s)?).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(s)
    }
    pub async fn productivity_delete_schedule(
        &self,
        project: &str,
        key: &str,
        revision: i64,
    ) -> Result<()> {
        let mut tx = begin_productivity(self).await?;
        product_project(&mut tx, project).await?;
        let schedule = read_schedule(&mut tx, project, key).await?;
        if schedule.revision != revision || schedule.active_occurrence.is_some() {
            return Err(Error::Conflict);
        }
        sqlx::query("DELETE FROM productivity_schedules WHERE project_id=$1 AND id=$2")
            .bind(project)
            .bind(key)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        product_change(&mut tx, project, "schedule", key, true, Value::Null).await?;
        tx.commit().await.map_err(storage_error)
    }
    /// Claims at most one due occurrence. Persisted claim blocks all other workers.
    pub async fn productivity_claim_schedule(
        &self,
        project: Option<&str>,
        key: Option<&str>,
        force: bool,
        revision: Option<i64>,
    ) -> Result<Option<ProductSchedule>> {
        let mut tx = begin_productivity(self).await?;
        let rows=sqlx::query("SELECT payload FROM productivity_schedules WHERE paused=0 AND ($1 IS NULL OR project_id=$1) AND ($2 IS NULL OR id=$2) AND (next_run<=$3 OR $4=1) AND ($5 IS NULL OR revision=$5) ORDER BY next_run,id LIMIT 501")
            .bind(project).bind(key).bind(now()).bind(i64::from(force)).bind(revision).fetch_all(&mut *tx).await.map_err(storage_error)?;
        for row in rows {
            let mut s: ProductSchedule =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            if s.active_occurrence.is_some() || s.next_run.is_none() {
                continue;
            }
            match crate::workspace_management::order_lock(&mut tx, &s.template.order_id).await {
                Ok(_) => {}
                Err(Error::NotFound) => continue,
                Err(error) => return Err(error),
            }
            let row = sqlx::query("SELECT payload FROM orders WHERE project_id=$1 AND id=$2")
                .bind(&s.project_id)
                .bind(&s.template.order_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
            let order: Order = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let occurrence = id();
            let stamp = now();
            s.active_occurrence = Some(ScheduleOccurrence {
                workflow_request: s.template.workflow.as_ref().map(|template| {
                    let mut request = template.clone();
                    request.order_id = order.id.clone();
                    request.base_revision = order.revision;
                    request.idempotency_key = format!("schedule:{}:{occurrence}", s.id);
                    request.prompt = s.template.prompt.clone();
                    if s.template.agent_config.is_some() {
                        request.agent_config = s.template.agent_config.clone();
                    }
                    request
                }),
                id: occurrence.clone(),
                due_at: s.next_run.unwrap_or(stamp),
                claimed_at: stamp,
                request: CreateRun {
                    agent_config: s.template.agent_config.clone(),
                    model_binding: None,
                    output: None,
                    order_id: order.id,
                    base_revision: order.revision,
                    idempotency_key: format!("schedule:{}:{occurrence}", s.id),
                    prompt: s.template.prompt.clone(),
                    provider_id: s.template.provider_id.clone(),
                    provider_revision: s.template.provider_revision,
                    budgets: s.template.budgets.clone(),
                    access_mode: s.template.access_mode.clone(),
                    folders: s.template.folders.clone(),
                    environment: s.template.environment.clone(),
                    mcp_tools: s.template.mcp_tools.clone(),
                    native_tools: s.template.native_tools.clone(),
                },
                run_id: None,
                uncertain: false,
            });
            s.revision += 1;
            s.updated_at = stamp;
            save_schedule(&mut tx, &s).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(Some(s));
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(None)
    }
    pub async fn productivity_schedule_dispatch_result(
        &self,
        project: &str,
        key: &str,
        occurrence: &str,
        run: Option<&RunRecord>,
    ) -> Result<ProductSchedule> {
        let mut tx = begin_productivity(self).await?;
        let mut s = read_schedule(&mut tx, project, key).await?;
        let active = s.active_occurrence.as_mut().ok_or(Error::Conflict)?;
        if active.id != occurrence {
            return Err(Error::Conflict);
        }
        if let Some(run) = run {
            if run.spec.order_id != active.request.order_id || run.spec.project_id != project {
                return Err(Error::Conflict);
            }
            let actual = sqlx::query(
                "SELECT id,request_hash FROM runs WHERE order_id=$1 AND idempotency_key=$2",
            )
            .bind(&active.request.order_id)
            .bind(&active.request.idempotency_key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::Conflict)?;
            if actual.try_get::<String, _>("id").map_err(storage_error)? != run.id
                || actual
                    .try_get::<String, _>("request_hash")
                    .map_err(storage_error)?
                    != occurrence_hash(active)
            {
                return Err(Error::Conflict);
            }
            if active.run_id.as_ref().is_some_and(|r| r != &run.id) {
                return Err(Error::Conflict);
            }
            active.run_id = Some(run.id.clone());
            active.uncertain = false;
            s.last_error = None;
        } else {
            active.uncertain = true;
            s.paused = true;
            s.last_error = Some("dispatch_outcome_requires_review".into());
        }
        s.revision += 1;
        s.updated_at = now();
        save_schedule(&mut tx, &s).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(s)
    }
    /// Reconciliation only observes actual durable runs. It never resends a request.
    pub async fn productivity_reconcile_schedules(&self) -> Result<usize> {
        let mut tx = begin_productivity(self).await?;
        let rows = sqlx::query("SELECT payload FROM productivity_schedules ORDER BY id")
            .fetch_all(&mut *tx)
            .await
            .map_err(storage_error)?;
        let mut count = 0;
        for row in rows {
            let mut s: ProductSchedule =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let Some(active) = &s.active_occurrence else {
                continue;
            };
            let runrow = sqlx::query(
                "SELECT payload,request_hash FROM runs WHERE order_id=$1 AND idempotency_key=$2",
            )
            .bind(&active.request.order_id)
            .bind(&active.request.idempotency_key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
            if let Some(row) = runrow {
                let stored: StoredRun =
                    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                let actual_hash: String = row.try_get("request_hash").map_err(storage_error)?;
                if actual_hash != occurrence_hash(active)
                    || stored.record.spec.project_id != s.project_id
                {
                    return Err(Error::Conflict);
                }
                if terminal(&stored.record.state) {
                    let consumed_at = active.due_at;
                    s.last_run_id = Some(stored.record.id);
                    s.active_occurrence = None;
                    s.next_run =
                        productivity_next_run(&s.recurrence, &s.timezone, now().max(consumed_at))?;
                    if s.next_run.is_none() {
                        s.paused = true;
                    }
                    s.last_error = None;
                } else if active.run_id.is_none() {
                    s.active_occurrence.as_mut().ok_or(Error::Conflict)?.run_id =
                        Some(stored.record.id);
                    s.active_occurrence
                        .as_mut()
                        .ok_or(Error::Conflict)?
                        .uncertain = false;
                } else {
                    continue;
                }
            } else if now().saturating_sub(active.claimed_at) >= 300_000 && !active.uncertain {
                s.active_occurrence
                    .as_mut()
                    .ok_or(Error::Conflict)?
                    .uncertain = true;
                s.paused = true;
                s.last_error = Some("dispatch_outcome_requires_review".into());
            } else {
                continue;
            }
            s.revision += 1;
            s.updated_at = now();
            save_schedule(&mut tx, &s).await?;
            count += 1;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(count)
    }
    pub async fn productivity_sync(
        &self,
        project: &str,
        after: i64,
        limit: i64,
    ) -> Result<ProductivitySyncPage> {
        if after < 0 || !(1..=500).contains(&limit) {
            return Err(Error::Validation("Invalid sync cursor".into()));
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        product_project(&mut tx, project).await?;
        let rows=sqlx::query("SELECT sequence,kind,object_id,deleted,payload,created_at FROM productivity_changes WHERE project_id=$1 AND sequence>$2 ORDER BY sequence LIMIT $3").bind(project).bind(after).bind(limit+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let more = rows.len() > limit as usize;
        let mut changes = Vec::new();
        for row in rows.into_iter().take(limit as usize) {
            changes.push(ProductivityChange {
                sequence: row.try_get("sequence").map_err(storage_error)?,
                project_id: project.into(),
                kind: row.try_get("kind").map_err(storage_error)?,
                object_id: row.try_get("object_id").map_err(storage_error)?,
                deleted: row.try_get::<i64, _>("deleted").map_err(storage_error)? == 1,
                payload: parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?,
                created_at: row.try_get("created_at").map_err(storage_error)?,
            });
        }
        Ok(ProductivitySyncPage {
            format_version: 1,
            next_cursor: changes.last().map_or(after, |c| c.sequence),
            changes,
            has_more: more,
        })
    }
    /// Human acknowledgement of an occurrence that never produced a durable run.
    /// Clears only the uncertain claim; does not repeat it or unpause the schedule.
    pub async fn productivity_abandon_uncertain_schedule(
        &self,
        project: &str,
        key: &str,
        revision: i64,
    ) -> Result<ProductSchedule> {
        let mut tx = begin_productivity(self).await?;
        product_project(&mut tx, project).await?;
        let mut s = read_schedule(&mut tx, project, key).await?;
        let active = s.active_occurrence.as_ref().ok_or(Error::Conflict)?;
        if s.revision != revision || !active.uncertain || !s.paused {
            return Err(Error::Conflict);
        }
        let exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM runs WHERE order_id=$1 AND idempotency_key=$2",
        )
        .bind(&active.request.order_id)
        .bind(&active.request.idempotency_key)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if exists != 0 {
            return Err(Error::Conflict);
        }
        s.active_occurrence = None;
        s.next_run = productivity_next_run(&s.recurrence, &s.timezone, now())?;
        s.last_error = Some("uncertain_occurrence_abandoned_by_owner".into());
        s.updated_at = now();
        s.revision += 1;
        save_schedule(&mut tx, &s).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(s)
    }
}
