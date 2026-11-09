//! Durable incomplete editor data with immutable conflict copies and CAS receipts.
use super::*;
use cowork_workflow_contracts::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftSaveReceipt {
    head_snapshot_id: String,
    snapshot_id: String,
    conflict: bool,
}

impl Store {
    /// Explicit archival, never deletion of an execution or idempotency receipt.
    pub async fn archive_workflow_journal(
        &self,
        project: &str,
        before_ms: i64,
        keep_snapshots: u32,
    ) -> Result<Value> {
        if before_ms < 0 || before_ms > now() || !(1..=127).contains(&keep_snapshots) {
            return Err(Error::Validation(
                "Invalid journal retention selection".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        require_project(&mut tx, project).await?;
        sqlx::query("UPDATE workflow_journal_quotas SET drafts=drafts WHERE project_id=$1")
            .bind(project)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let mut archived = Vec::<Value>::new();
        let drafts = sqlx::query_scalar::<_, String>(
            "SELECT payload FROM workflow_drafts WHERE project_id=$1",
        )
        .bind(project)
        .fetch_all(&mut *tx)
        .await
        .map_err(storage_error)?;
        for payload in drafts {
            let draft: WorkflowDraft = parse(&payload)?;
            if draft.content.archived
                && draft.updated_at <= before_ms
                && archive_marker(&mut tx, project, "draft", &draft.id).await?
            {
                archived.push(json!({"kind":"draft","id":draft.id,"sha256":draft.sha256}));
            }
            let rows=sqlx::query_scalar::<_,String>("SELECT payload FROM workflow_draft_snapshots s WHERE project_id=$1 AND draft_id=$2 AND NOT EXISTS(SELECT 1 FROM workflow_journal_archives a WHERE a.project_id=s.project_id AND a.kind='snapshot' AND a.object_id=s.id) ORDER BY id DESC LIMIT 4096").bind(project).bind(&draft.id).fetch_all(&mut *tx).await.map_err(storage_error)?;
            let archived_draft = draft.content.archived && draft.updated_at <= before_ms;
            for payload in rows.into_iter().skip(if archived_draft {
                0
            } else {
                keep_snapshots as usize
            }) {
                let snapshot: WorkflowDraftSnapshot = parse(&payload)?;
                // Conflict copies and the current head remain available in the active editor.
                if (archived_draft || snapshot.id != draft.snapshot_id)
                    && !snapshot.conflict
                    && snapshot.created_at <= before_ms
                    && archive_marker(&mut tx, project, "snapshot", &snapshot.id).await?
                {
                    archived
                        .push(json!({"kind":"snapshot","id":snapshot.id,"sha256":snapshot.sha256}));
                }
            }
        }
        let operations=sqlx::query_scalar::<_,String>("SELECT payload FROM workflow_operations o WHERE project_id=$1 AND state<>'pending' AND acknowledged_at IS NOT NULL AND acknowledged_at<=$2 AND NOT EXISTS(SELECT 1 FROM workflow_journal_archives a WHERE a.project_id=o.project_id AND a.kind='operation' AND a.object_id=o.id) LIMIT 4096").bind(project).bind(before_ms).fetch_all(&mut *tx).await.map_err(storage_error)?;
        for payload in operations {
            let operation: WorkflowOperation = parse(&payload)?;
            if archive_marker(&mut tx, project, "operation", &operation.id).await? {
                archived.push(json!({"kind":"operation","id":operation.id,"request_sha256":operation.request_sha256}));
            }
        }
        let drafts:i64=sqlx::query_scalar("SELECT COUNT(*) FROM workflow_drafts d WHERE project_id=$1 AND NOT EXISTS(SELECT 1 FROM workflow_journal_archives a WHERE a.project_id=d.project_id AND a.kind='draft' AND a.object_id=d.id)").bind(project).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let row=sqlx::query("SELECT COUNT(*) AS count,CAST(COALESCE(SUM(payload_bytes),0) AS BIGINT) AS bytes FROM workflow_draft_snapshots s WHERE project_id=$1 AND NOT EXISTS(SELECT 1 FROM workflow_journal_archives a WHERE a.project_id=s.project_id AND a.kind='snapshot' AND a.object_id=s.id)").bind(project).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let snapshots: i64 = row.try_get("count").map_err(storage_error)?;
        let bytes: i64 = row.try_get("bytes").map_err(storage_error)?;
        let operations:i64=sqlx::query_scalar("SELECT COUNT(*) FROM workflow_operations o WHERE project_id=$1 AND NOT EXISTS(SELECT 1 FROM workflow_journal_archives a WHERE a.project_id=o.project_id AND a.kind='operation' AND a.object_id=o.id)").bind(project).fetch_one(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE workflow_journal_quotas SET drafts=$2,snapshots=$3,snapshot_bytes=$4,operations=$5 WHERE project_id=$1").bind(project).bind(drafts).bind(snapshots).bind(bytes).bind(operations).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"format_version":1,"project_id":project,"archived":archived,"active_quota":{"drafts":drafts,"snapshots":snapshots,"snapshot_bytes":bytes,"operations":operations},"receipts_retained":true,"execution_repeated":false}),
        )
    }
    pub async fn save_workflow_draft(
        &self,
        project: &str,
        input: &SaveWorkflowDraft,
    ) -> Result<WorkflowDraftSave> {
        valid_text(&input.id, 128)?;
        valid_text(&input.idempotency_key, 128)?;
        valid_text(&input.content.editor_format, 64)?;
        if input.base_revision < 0 || input.base_revision.checked_add(1).is_none() {
            return Err(Error::Validation("Invalid draft basis".into()));
        }
        let content_bytes = json_string(&input.content)?.len();
        if content_bytes > 1_000_000 {
            return Err(Error::Validation("Draft exceeds one megabyte".into()));
        }
        let digest = canonical_hash(input);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        require_project(&mut tx, project).await?;
        if let Some(result) =
            save_receipt(&mut tx, project, &input.id, &input.idempotency_key, &digest).await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(result);
        }
        let existing =
            sqlx::query_scalar::<_, String>("SELECT payload FROM workflow_drafts WHERE id=$1")
                .bind(&input.id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        if existing.is_none() && input.base_revision != 0 {
            return Err(Error::NotFound);
        }
        if let Some(origin) = &input.content.origin {
            if origin.base_revision < 1 {
                return Err(Error::Validation("Invalid workflow origin".into()));
            }
            workflows::registry::read_version(
                &mut tx,
                project,
                &origin.workflow_id,
                &origin.version_id,
            )
            .await?;
        }
        let at = now();
        let mut snapshot = WorkflowDraftSnapshot {
            id: id(),
            draft_id: input.id.clone(),
            project_id: project.into(),
            base_revision: input.base_revision,
            revision: input.base_revision + 1,
            sha256: canonical_hash(&input.content),
            content: input.content.clone(),
            conflict: false,
            created_at: at,
        };
        let candidate = WorkflowDraft {
            id: input.id.clone(),
            project_id: project.into(),
            revision: 1,
            snapshot_id: snapshot.id.clone(),
            sha256: snapshot.sha256.clone(),
            content: input.content.clone(),
            updated_at: at,
        };
        let inserted = sqlx::query("INSERT INTO workflow_drafts(id,project_id,revision,snapshot_id,payload) VALUES($1,$2,1,$3,$4) ON CONFLICT(id) DO NOTHING")
            .bind(&candidate.id).bind(project).bind(&candidate.snapshot_id).bind(json_string(&candidate)?)
            .execute(&mut *tx).await.map_err(storage_error)?.rows_affected() == 1;
        if sqlx::query("UPDATE workflow_drafts SET revision=revision WHERE id=$1 AND project_id=$2")
            .bind(&input.id)
            .bind(project)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
            != 1
        {
            return Err(Error::NotFound);
        }
        if let Some(result) =
            save_receipt(&mut tx, project, &input.id, &input.idempotency_key, &digest).await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(result);
        }
        let before = read_draft(&mut tx, project, &input.id).await?;
        if !inserted {
            verify_head_snapshot(&mut tx, &before).await?;
        }
        let snapshot_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM workflow_draft_snapshots s WHERE draft_id=$1 AND NOT EXISTS(SELECT 1 FROM workflow_journal_archives a WHERE a.project_id=s.project_id AND a.kind='snapshot' AND a.object_id=s.id)",
        )
        .bind(&input.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if snapshot_count >= 128 {
            return Err(Error::BudgetExceeded);
        }
        if !input.content.archived && sqlx::query("DELETE FROM workflow_journal_archives WHERE project_id=$1 AND kind='draft' AND object_id=$2").bind(project).bind(&input.id).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()>0 {
            reserve_quota(&mut tx,project,1,0,0,0).await?;
        }
        snapshot.conflict = if inserted {
            false
        } else {
            before.revision != input.base_revision
        };
        snapshot.revision = if snapshot.conflict {
            before.revision
        } else if inserted {
            1
        } else {
            before.revision + 1
        };
        let snapshot_payload = json_string(&snapshot)?;
        reserve_quota(
            &mut tx,
            project,
            i64::from(inserted),
            1,
            snapshot_payload.len() as i64,
            0,
        )
        .await?;
        let draft = if snapshot.conflict {
            before
        } else {
            WorkflowDraft {
                revision: snapshot.revision,
                snapshot_id: snapshot.id.clone(),
                ..candidate
            }
        };
        sqlx::query("INSERT INTO workflow_draft_snapshots(id,draft_id,project_id,payload_bytes,payload) VALUES($1,$2,$3,$4,$5)")
            .bind(&snapshot.id).bind(&snapshot.draft_id).bind(project).bind(snapshot_payload.len() as i64).bind(snapshot_payload)
            .execute(&mut *tx).await.map_err(storage_error)?;
        if !snapshot.conflict {
            sqlx::query("UPDATE workflow_drafts SET revision=$1,snapshot_id=$2,payload=$3 WHERE id=$4 AND project_id=$5")
                .bind(draft.revision).bind(&draft.snapshot_id).bind(json_string(&draft)?).bind(&draft.id).bind(project)
                .execute(&mut *tx).await.map_err(storage_error)?;
        }
        let stored_receipt = DraftSaveReceipt {
            head_snapshot_id: draft.snapshot_id.clone(),
            snapshot_id: snapshot.id.clone(),
            conflict: snapshot.conflict,
        };
        let result = if snapshot.conflict {
            WorkflowDraftSave::Conflict { draft, snapshot }
        } else {
            WorkflowDraftSave::Saved { draft, snapshot }
        };
        sqlx::query("INSERT INTO workflow_draft_saves(draft_id,request_key,request_sha256,payload) VALUES($1,$2,$3,$4)")
            .bind(&input.id).bind(&input.idempotency_key).bind(digest).bind(json_string(&stored_receipt)?)
            .execute(&mut *tx).await.map_err(storage_error)?;
        // Conflict is a committed receipt, not a rolled-back SQL error.
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn workflow_draft(&self, project: &str, draft: &str) -> Result<WorkflowDraft> {
        let mut tx = self.snapshot_transaction().await?;
        let result = read_draft(&mut tx, project, draft).await?;
        verify_head_snapshot(&mut tx, &result).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn workflow_draft_snapshot(
        &self,
        project: &str,
        draft: &str,
        snapshot: &str,
    ) -> Result<WorkflowDraftSnapshot> {
        let mut tx = self.snapshot_transaction().await?;
        let result = read_snapshot(&mut tx, project, draft, snapshot).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn list_workflow_drafts(
        &self,
        project: &str,
        offset: u32,
        limit: u32,
    ) -> Result<WorkflowDraftPage> {
        page(offset, limit)?;
        let mut tx = self.snapshot_transaction().await?;
        require_project(&mut tx, project).await?;
        let total = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_drafts WHERE project_id=$1")
            .bind(project)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM workflow_drafts WHERE project_id=$1 ORDER BY id LIMIT $2 OFFSET $3",
        )
        .bind(project)
        .bind(i64::from(limit) + 1)
        .bind(i64::from(offset))
        .fetch_all(&mut *tx)
        .await
        .map_err(storage_error)?;
        let next_offset = (ids.len() > limit as usize).then_some(offset + limit);
        let mut items = Vec::new();
        for id in ids.into_iter().take(limit as usize) {
            let draft = read_draft(&mut tx, project, &id).await?;
            verify_head_snapshot(&mut tx, &draft).await?;
            items.push(draft);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(WorkflowDraftPage {
            items,
            total,
            next_offset,
        })
    }
    pub async fn list_workflow_draft_snapshots(
        &self,
        project: &str,
        draft: &str,
        offset: u32,
        limit: u32,
    ) -> Result<WorkflowDraftSnapshotPage> {
        page(offset, limit)?;
        let mut tx = self.snapshot_transaction().await?;
        let head = read_draft(&mut tx, project, draft).await?;
        verify_head_snapshot(&mut tx, &head).await?;
        let total = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_draft_snapshots WHERE draft_id=$1 AND project_id=$2",
        )
        .bind(draft)
        .bind(project)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let rows = sqlx::query_scalar::<_,String>("SELECT payload FROM workflow_draft_snapshots WHERE draft_id=$1 AND project_id=$2 ORDER BY id LIMIT $3 OFFSET $4")
            .bind(draft).bind(project).bind(i64::from(limit)+1).bind(i64::from(offset)).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let next_offset = (rows.len() > limit as usize).then_some(offset + limit);
        let items = rows
            .into_iter()
            .take(limit as usize)
            .map(|s| {
                let snapshot: WorkflowDraftSnapshot = parse(&s)?;
                verify_snapshot(&snapshot, project, draft, &snapshot.id)?;
                Ok(WorkflowDraftSnapshotSummary::from(&snapshot))
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(WorkflowDraftSnapshotPage {
            items,
            total,
            next_offset,
        })
    }
}
async fn archive_marker(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    kind: &str,
    object: &str,
) -> Result<bool> {
    Ok(sqlx::query("INSERT INTO workflow_journal_archives(project_id,kind,object_id,archived_at) VALUES($1,$2,$3,$4) ON CONFLICT(project_id,kind,object_id) DO NOTHING").bind(project).bind(kind).bind(object).bind(now()).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()==1)
}
pub(super) async fn require_project(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
) -> Result<()> {
    if sqlx::query_scalar::<_, String>("SELECT id FROM projects WHERE id=$1")
        .bind(project)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .is_none()
    {
        return Err(Error::NotFound);
    }
    Ok(())
}
pub(super) fn page(offset: u32, limit: u32) -> Result<()> {
    if limit == 0 || limit > 100 || offset > 1_000_000 {
        return Err(Error::Validation("Invalid pagination".into()));
    }
    Ok(())
}
pub(super) async fn reserve_quota(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    drafts: i64,
    snapshots: i64,
    bytes: i64,
    operations: i64,
) -> Result<()> {
    sqlx::query("INSERT INTO workflow_journal_quotas(project_id) VALUES($1) ON CONFLICT(project_id) DO NOTHING").bind(project).execute(&mut **tx).await.map_err(storage_error)?;
    if sqlx::query("UPDATE workflow_journal_quotas SET drafts=drafts+$2,snapshots=snapshots+$3,snapshot_bytes=snapshot_bytes+$4,operations=operations+$5 WHERE project_id=$1 AND drafts+$2<=32 AND snapshots+$3<=4096 AND snapshot_bytes+$4<=33554432 AND operations+$5<=512")
        .bind(project).bind(drafts).bind(snapshots).bind(bytes).bind(operations).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1 { return Err(Error::BudgetExceeded); }
    Ok(())
}
async fn save_receipt(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    draft: &str,
    key: &str,
    digest: &str,
) -> Result<Option<WorkflowDraftSave>> {
    let row = sqlx::query("SELECT s.request_sha256,s.payload FROM workflow_draft_saves s JOIN workflow_drafts d ON d.id=s.draft_id WHERE d.project_id=$1 AND s.draft_id=$2 AND s.request_key=$3")
        .bind(project).bind(draft).bind(key).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    match row {
        None => Ok(None),
        Some(row) => {
            if row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != digest
            {
                return Err(Error::Conflict);
            }
            let stored: DraftSaveReceipt =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let snapshot = read_snapshot(tx, project, draft, &stored.snapshot_id).await?;
            let head = read_snapshot(tx, project, draft, &stored.head_snapshot_id).await?;
            if head.conflict
                || snapshot.conflict != stored.conflict
                || (!stored.conflict && head.id != snapshot.id)
                || (stored.conflict && head.revision != snapshot.revision)
                || canonical_hash(&SaveWorkflowDraft {
                    id: draft.into(),
                    base_revision: snapshot.base_revision,
                    idempotency_key: key.into(),
                    content: snapshot.content.clone(),
                }) != digest
            {
                return Err(Error::Forbidden);
            }
            let draft = WorkflowDraft {
                id: draft.into(),
                project_id: project.into(),
                revision: head.revision,
                snapshot_id: head.id,
                sha256: head.sha256,
                content: head.content,
                updated_at: head.created_at,
            };
            let receipt = if stored.conflict {
                WorkflowDraftSave::Conflict { draft, snapshot }
            } else {
                WorkflowDraftSave::Saved { draft, snapshot }
            };
            Ok(Some(receipt))
        }
    }
}
pub(super) async fn read_draft(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    draft: &str,
) -> Result<WorkflowDraft> {
    let row = sqlx::query(
        "SELECT payload,revision,snapshot_id FROM workflow_drafts WHERE project_id=$1 AND id=$2",
    )
    .bind(project)
    .bind(draft)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .ok_or(Error::NotFound)?;
    let result: WorkflowDraft =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if result.id != draft
        || result.project_id != project
        || result.revision < 1
        || result.sha256 != canonical_hash(&result.content)
        || result.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || result.snapshot_id
            != row
                .try_get::<String, _>("snapshot_id")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(result)
}
pub(super) async fn read_snapshot(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    draft: &str,
    snapshot: &str,
) -> Result<WorkflowDraftSnapshot> {
    let row=sqlx::query("SELECT payload,payload_bytes FROM workflow_draft_snapshots WHERE project_id=$1 AND draft_id=$2 AND id=$3").bind(project).bind(draft).bind(snapshot).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let payload: String = row.try_get("payload").map_err(storage_error)?;
    if payload.len() as i64
        != row
            .try_get::<i64, _>("payload_bytes")
            .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let result: WorkflowDraftSnapshot = parse(&payload)?;
    verify_snapshot(&result, project, draft, snapshot)?;
    Ok(result)
}
fn verify_snapshot(
    snapshot: &WorkflowDraftSnapshot,
    project: &str,
    draft: &str,
    id: &str,
) -> Result<()> {
    if snapshot.id != id
        || snapshot.project_id != project
        || snapshot.draft_id != draft
        || snapshot.sha256 != canonical_hash(&snapshot.content)
        || snapshot.revision < 1
        || snapshot.base_revision < 0
        || snapshot.created_at < 0
        || (!snapshot.conflict && snapshot.base_revision.checked_add(1) != Some(snapshot.revision))
        || (snapshot.conflict && snapshot.base_revision == snapshot.revision)
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

async fn verify_head_snapshot(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    draft: &WorkflowDraft,
) -> Result<()> {
    let snapshot = read_snapshot(tx, &draft.project_id, &draft.id, &draft.snapshot_id).await?;
    if snapshot.conflict
        || snapshot.revision != draft.revision
        || snapshot.sha256 != draft.sha256
        || snapshot.created_at != draft.updated_at
        || canonical_hash(&snapshot.content) != canonical_hash(&draft.content)
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
