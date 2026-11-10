//! Read-only native schedule evidence from the append-only productivity log.
use super::*;
pub use cowork_workflow_contracts::{
    NativeDeletedSchedule, NativeScheduleHistory, NativeScheduleHistoryEntry,
};
impl Store {
    pub async fn native_skill_history(
        &self,
        project: &str,
        skill: &str,
        before: Option<i64>,
        limit: i64,
    ) -> Result<NativeSkillHistory> {
        if !(1..=100).contains(&limit) || before.is_some_and(|v| v < 1) {
            return Err(Error::Validation("Invalid skill history cursor".into()));
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::productivity::product_project(&mut tx, project).await?;
        let rows=sqlx::query("SELECT sequence,payload,deleted,created_at FROM productivity_changes WHERE project_id=$1 AND object_id=$2 AND kind='skill' AND ($3 IS NULL OR sequence<$3) ORDER BY sequence DESC LIMIT $4").bind(project).bind(skill).bind(before).bind(limit+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let more = rows.len() > limit as usize;
        let mut items = Vec::new();
        for row in rows.into_iter().take(limit as usize) {
            let deleted = row.try_get::<i64, _>("deleted").map_err(storage_error)? != 0;
            items.push(NativeSkillRevision {
                sequence: row.try_get("sequence").map_err(storage_error)?,
                recorded_at: row.try_get("created_at").map_err(storage_error)?,
                deleted,
                skill: if deleted {
                    None
                } else {
                    Some(parse(
                        &row.try_get::<String, _>("payload").map_err(storage_error)?,
                    )?)
                },
            });
        }
        let next_cursor = if more {
            items.last().map(|v| v.sequence)
        } else {
            None
        };
        Ok(NativeSkillHistory { items, next_cursor })
    }
    pub async fn native_deleted_schedules(
        &self,
        project: &str,
    ) -> Result<Vec<NativeDeletedSchedule>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::productivity::product_project(&mut tx, project).await?;
        let rows=sqlx::query("SELECT c.object_id FROM productivity_changes c WHERE c.project_id=$1 AND c.kind='schedule' AND c.deleted=1 AND NOT EXISTS (SELECT 1 FROM productivity_schedules s WHERE s.id=c.object_id AND s.project_id=c.project_id) GROUP BY c.object_id ORDER BY MAX(c.sequence) DESC LIMIT 500").bind(project).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let mut result = Vec::new();
        for row in rows {
            let id: String = row.try_get("object_id").map_err(storage_error)?;
            let raw:Option<String>=sqlx::query_scalar("SELECT payload FROM productivity_changes WHERE project_id=$1 AND object_id=$2 AND kind='schedule' AND deleted=0 ORDER BY sequence DESC LIMIT 1").bind(project).bind(&id).fetch_optional(&mut *tx).await.map_err(storage_error)?;
            let name = raw
                .as_deref()
                .map(parse::<ProductSchedule>)
                .transpose()?
                .map_or_else(|| id.clone(), |v| v.name);
            result.push(NativeDeletedSchedule { id, name });
        }
        Ok(result)
    }
    pub async fn native_schedule_original(
        &self,
        project: &str,
        input: &WriteSchedule,
        key: &str,
    ) -> Result<Option<ProductSchedule>> {
        valid_text(key, 128)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::productivity::product_project(&mut tx, project).await?;
        let row=sqlx::query("SELECT request_sha256,receipt FROM workspace_management_operations WHERE scope=$1 AND request_key=$2").bind(format!("native-schedule:{project}")).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if let Some(row) = row {
            if row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != canonical_hash(input)
            {
                return Err(Error::Conflict);
            }
            return Ok(Some(parse(
                &row.try_get::<String, _>("receipt").map_err(storage_error)?,
            )?));
        }
        Ok(None)
    }
    /// Checkpoints retain original requests, including lost-dispatch outcomes. Reading
    /// this method never creates, repeats, reconciles or otherwise mutates a run.
    pub async fn native_schedule_history(
        &self,
        project: &str,
        schedule: &str,
        before: Option<i64>,
        limit: i64,
    ) -> Result<NativeScheduleHistory> {
        if !(1..=100).contains(&limit) || before.is_some_and(|v| v < 1) {
            return Err(Error::Validation("Invalid history cursor".into()));
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::productivity::product_project(&mut tx, project).await?;
        let rows=sqlx::query("SELECT sequence,payload,deleted,created_at FROM productivity_changes WHERE project_id=$1 AND object_id=$2 AND kind='schedule' AND ($3 IS NULL OR sequence<$3) ORDER BY sequence DESC LIMIT $4")
            .bind(project).bind(schedule).bind(before).bind(limit+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let more = rows.len() > limit as usize;
        let mut items = Vec::new();
        for row in rows.into_iter().take(limit as usize) {
            let deleted = row.try_get::<i64, _>("deleted").map_err(storage_error)? != 0;
            let snapshot: Option<ProductSchedule> = if deleted {
                None
            } else {
                Some(parse(
                    &row.try_get::<String, _>("payload").map_err(storage_error)?,
                )?)
            };
            let occurrence = snapshot.as_ref().and_then(|v| v.active_occurrence.clone());
            let mut run = None;
            if let Some(active) = &occurrence {
                if let Some(row)=sqlx::query("SELECT payload,request_hash FROM runs WHERE order_id=$1 AND idempotency_key=$2").bind(&active.request.order_id).bind(&active.request.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)? {
                    if row.try_get::<String,_>("request_hash").map_err(storage_error)?!=crate::productivity::occurrence_hash(active){return Err(Error::Conflict);}
                    let stored:StoredRun=parse(&row.try_get::<String,_>("payload").map_err(storage_error)?)?;
                    if stored.record.spec.project_id != project { return Err(Error::Conflict); }
                    run=Some(stored.record);
                }
            } else if let Some(id) = snapshot.as_ref().and_then(|v| v.last_run_id.as_ref())
                && let Some(raw) =
                    sqlx::query_scalar::<_, String>("SELECT payload FROM runs WHERE id=$1")
                        .bind(id)
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(storage_error)?
            {
                let record = parse::<StoredRun>(&raw)?.record;
                if record.spec.project_id != project {
                    return Err(Error::Conflict);
                }
                run = Some(record);
            }
            items.push(NativeScheduleHistoryEntry {
                sequence: row.try_get("sequence").map_err(storage_error)?,
                recorded_at: row.try_get("created_at").map_err(storage_error)?,
                schedule_revision: snapshot.as_ref().map(|v| v.revision),
                deleted,
                paused: snapshot.as_ref().map(|v| v.paused),
                next_run: snapshot.as_ref().and_then(|v| v.next_run),
                occurrence,
                run,
            });
        }
        let next_cursor = if more {
            items.last().map(|v| v.sequence)
        } else {
            None
        };
        Ok(NativeScheduleHistory { items, next_cursor })
    }
}
