//! Owner-scoped answer drafts. Only a frozen intent may join the core answer tx.
use super::*;
use cowork_workflow_contracts::*;
use serde::{Deserialize, Serialize};
const OWNER: &str = "single_owner_preview";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveReceipt {
    head: String,
    incoming: String,
    conflict: bool,
}
pub(crate) struct WorkflowInputIntentWrite {
    intent: WorkflowInputAnswerIntent,
    receipt: WorkflowInputReceipt,
}
fn answer(value: &str, complete: bool) -> Result<()> {
    if value.len() > 32_000 || value.contains('\0') || (complete && value.trim().is_empty()) {
        return Err(Error::Validation("Answer must be at most 32000 UTF-8 bytes, without NUL; a submitted answer must contain text".into()));
    }
    Ok(())
}
fn fp(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::Validation("Invalid input fingerprint".into()));
    }
    Ok(())
}
fn scope_hash(run: &str, request: &str, value: &impl Serialize) -> String {
    canonical_hash(
        &json!({"format":"cowork-workflow-human-answer-v1","owner":OWNER,"run_id":run,"request_id":request,"request":value}),
    )
}
fn intent_hash(intent: &WorkflowInputAnswerIntent) -> String {
    scope_hash(
        &intent.run_id,
        &intent.request_id,
        &json!({"project_id":intent.project_id,"submission":intent.submission,"draft_snapshot":intent.draft_snapshot}),
    )
}
fn prepare_hash(intent: &WorkflowInputAnswerIntent) -> String {
    scope_hash(
        &intent.run_id,
        &intent.request_id,
        &PrepareWorkflowInputAnswer {
            fingerprint: intent.fingerprint.clone(),
            base_draft_revision: intent.draft_snapshot.revision,
            idempotency_key: intent.submission.idempotency_key.clone(),
        },
    )
}
fn state_name(state: &WorkflowInputAnswerIntentState) -> &'static str {
    match state {
        WorkflowInputAnswerIntentState::Pending => "pending",
        WorkflowInputAnswerIntentState::Accepted => "accepted",
        WorkflowInputAnswerIntentState::Rejected => "rejected",
    }
}
async fn scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &str,
    request: &str,
    expected: Option<&str>,
) -> Result<(StoredRun, StoredWorkflow, WorkflowInputView)> {
    valid_text(run, 128)?;
    valid_text(request, 128)?;
    let row = sqlx::query("SELECT order_id,revision,state,payload FROM runs WHERE id=$1")
        .bind(run)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    let parent: StoredRun = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if parent.record.id != run
        || parent.record.spec.order_id
            != row
                .try_get::<String, _>("order_id")
                .map_err(storage_error)?
        || parent.record.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || super::state_name(&parent.record.state)
            != row.try_get::<String, _>("state").map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let project = sqlx::query_scalar::<_, String>("SELECT project_id FROM orders WHERE id=$1")
        .bind(&parent.record.spec.order_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    if project != parent.record.spec.project_id {
        return Err(Error::Forbidden);
    }
    let workflow = workflows::runs::load(tx, &parent.record).await?;
    let q = workflow.inputs.get(request).ok_or(Error::NotFound)?;
    let view = workflows::inputs::read_binding(
        &parent,
        &workflow,
        request,
        expected.unwrap_or(&q.fingerprint),
    )?;
    Ok((parent, workflow, view))
}
/// A committed cancellation remains authoritative after conservative recovery.
/// Callers hold registry/run locks for writes; reads use one database snapshot.
pub(crate) async fn known_cancellation(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &RunRecord,
) -> Result<bool> {
    let rows = sqlx::query(
        "SELECT id,sequence,payload FROM events WHERE run_id=$1 AND sequence<=$2 ORDER BY sequence",
    )
    .bind(&parent.id)
    .bind(parent.last_sequence)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    if parent.last_sequence < 0 || rows.len() as i64 != parent.last_sequence {
        return Err(Error::Forbidden);
    }
    let mut canceled = false;
    for (index, row) in rows.into_iter().enumerate() {
        let payload: String = row.try_get("payload").map_err(storage_error)?;
        let event: RunEvent = serde_json::from_str(&payload).map_err(|_| Error::Forbidden)?;
        let sequence: i64 = row.try_get("sequence").map_err(storage_error)?;
        if event.id != row.try_get::<String, _>("id").map_err(storage_error)?
            || event.run_id != parent.id
            || event.sequence != sequence
            || sequence != index as i64 + 1
            || sequence <= 0
            || sequence > parent.last_sequence
        {
            return Err(Error::Forbidden);
        }
        if event.kind == "cancel_requested" {
            if event.payload.as_object().is_none_or(|p| {
                p.len() != 1 || p.get("state").and_then(Value::as_str) != Some("cancel_requested")
            }) {
                return Err(Error::Forbidden);
            }
            canceled = true;
        }
    }
    Ok(canceled)
}
async fn writable(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &StoredRun,
    workflow: &StoredWorkflow,
    view: &WorkflowInputView,
    prepare: bool,
    observed_at: i64,
) -> Result<()> {
    let entry = workflows::registry::read_entry(
        tx,
        &parent.record.spec.project_id,
        &workflow.binding.workflow_id,
    )
    .await?;
    if entry.revoked {
        return Err(Error::WorkflowRevoked);
    }
    if known_cancellation(tx, &parent.record).await? {
        return Err(Error::Forbidden);
    }
    if view.state == WorkflowInputState::Answered
        || observed_at >= view.expires_at
        || (view.state == WorkflowInputState::Closed
            && !matches!(
                view.closed_reason.as_deref(),
                Some("interrupted" | "stale_lease")
            ))
    {
        return Err(Error::Forbidden);
    }
    if prepare {
        if !view.actionable {
            return Err(Error::Forbidden);
        }
    } else if !matches!(
        parent.record.state,
        RunState::Running | RunState::Interrupted
    ) {
        return Err(Error::Forbidden);
    }
    Ok(())
}
fn draft_from(snapshot: &WorkflowInputDraftSnapshot) -> WorkflowInputDraft {
    WorkflowInputDraft {
        run_id: snapshot.run_id.clone(),
        project_id: snapshot.project_id.clone(),
        request_id: snapshot.request_id.clone(),
        fingerprint: snapshot.fingerprint.clone(),
        owner: OWNER.into(),
        revision: snapshot.revision,
        answer: snapshot.answer.clone(),
        answer_sha256: snapshot.answer_sha256.clone(),
        snapshot_id: Some(snapshot.id.clone()),
        updated_at: snapshot.created_at,
    }
}
async fn read_snapshot(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &str,
    request: &str,
    fingerprint: &str,
    snapshot: &str,
) -> Result<WorkflowInputDraftSnapshot> {
    let row=sqlx::query("SELECT project_id,payload_bytes,payload FROM workflow_input_draft_snapshots WHERE id=$1 AND run_id=$2 AND request_id=$3 AND owner=$4").bind(snapshot).bind(run).bind(request).bind(OWNER).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let payload: String = row.try_get("payload").map_err(storage_error)?;
    let result: WorkflowInputDraftSnapshot = parse(&payload)?;
    if payload.len() as i64
        != row
            .try_get::<i64, _>("payload_bytes")
            .map_err(storage_error)?
        || result.id != snapshot
        || result.run_id != run
        || result.request_id != request
        || result.owner != OWNER
        || result.fingerprint != fingerprint
        || result.project_id
            != row
                .try_get::<String, _>("project_id")
                .map_err(storage_error)?
        || result.answer_sha256 != hash(result.answer.as_bytes())
        || result.revision < 1
        || result.base_revision < 0
        || result.base_revision.checked_add(1).is_none()
        || result.created_at < 0
        || (!result.conflict && result.base_revision + 1 != result.revision)
        || (result.conflict && result.base_revision == result.revision)
        || answer(&result.answer, false).is_err()
    {
        return Err(Error::Forbidden);
    }
    Ok(result)
}
async fn read_head(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &RunRecord,
    view: &WorkflowInputView,
) -> Result<Option<WorkflowInputDraft>> {
    let row=sqlx::query("SELECT project_id,revision,snapshot_id,payload FROM workflow_input_drafts WHERE run_id=$1 AND request_id=$2 AND owner=$3").bind(&parent.id).bind(&view.request_id).bind(OWNER).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let draft: WorkflowInputDraft =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if draft.run_id != parent.id
        || draft.project_id != parent.spec.project_id
        || draft.project_id
            != row
                .try_get::<String, _>("project_id")
                .map_err(storage_error)?
        || draft.request_id != view.request_id
        || draft.fingerprint != view.fingerprint
        || draft.owner != OWNER
        || draft.revision < 1
        || draft.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || draft.snapshot_id.as_deref()
            != Some(
                row.try_get::<String, _>("snapshot_id")
                    .map_err(storage_error)?
                    .as_str(),
            )
    {
        return Err(Error::Forbidden);
    }
    let snapshot = read_snapshot(
        tx,
        &parent.id,
        &view.request_id,
        &view.fingerprint,
        draft.snapshot_id.as_deref().ok_or(Error::Forbidden)?,
    )
    .await?;
    if snapshot.conflict
        || snapshot.project_id != parent.spec.project_id
        || canonical_hash(&draft_from(&snapshot)) != canonical_hash(&draft)
    {
        return Err(Error::Forbidden);
    }
    Ok(Some(draft))
}
async fn quota(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    heads: i64,
    snapshots: i64,
    bytes: i64,
    intents: i64,
) -> Result<()> {
    sqlx::query("INSERT INTO workflow_input_quotas(project_id) VALUES($1) ON CONFLICT(project_id) DO NOTHING").bind(project).execute(&mut **tx).await.map_err(storage_error)?;
    if sqlx::query("UPDATE workflow_input_quotas SET heads=heads+$2,snapshots=snapshots+$3,snapshot_bytes=snapshot_bytes+$4,intents=intents+$5 WHERE project_id=$1 AND heads+$2<=512 AND snapshots+$3<=4096 AND snapshot_bytes+$4<=33554432 AND intents+$5<=512").bind(project).bind(heads).bind(snapshots).bind(bytes).bind(intents).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1 { return Err(Error::BudgetExceeded); }
    Ok(())
}
async fn replay_save(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &RunRecord,
    view: &WorkflowInputView,
    input: &SaveWorkflowInputDraft,
) -> Result<Option<WorkflowInputDraftSave>> {
    let row=sqlx::query("SELECT request_sha256,payload FROM workflow_input_draft_saves WHERE run_id=$1 AND request_id=$2 AND owner=$3 AND request_key=$4").bind(&parent.id).bind(&view.request_id).bind(OWNER).bind(&input.idempotency_key).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let digest = scope_hash(&parent.id, &view.request_id, input);
    if row
        .try_get::<String, _>("request_sha256")
        .map_err(storage_error)?
        != digest
    {
        return Err(Error::Conflict);
    }
    let receipt: SaveReceipt = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    let head = read_snapshot(
        tx,
        &parent.id,
        &view.request_id,
        &view.fingerprint,
        &receipt.head,
    )
    .await?;
    let incoming = read_snapshot(
        tx,
        &parent.id,
        &view.request_id,
        &view.fingerprint,
        &receipt.incoming,
    )
    .await?;
    let original = SaveWorkflowInputDraft {
        fingerprint: incoming.fingerprint.clone(),
        base_revision: incoming.base_revision,
        idempotency_key: input.idempotency_key.clone(),
        answer: incoming.answer.clone(),
    };
    if head.project_id != parent.spec.project_id
        || incoming.project_id != parent.spec.project_id
        || head.conflict
        || incoming.conflict != receipt.conflict
        || (!receipt.conflict && head.id != incoming.id)
        || (receipt.conflict && head.revision != incoming.revision)
        || scope_hash(&parent.id, &view.request_id, &original) != digest
    {
        return Err(Error::Forbidden);
    }
    let draft = draft_from(&head);
    Ok(Some(if receipt.conflict {
        WorkflowInputDraftSave::Conflict {
            draft,
            snapshot: incoming,
        }
    } else {
        WorkflowInputDraftSave::Saved {
            draft,
            snapshot: incoming,
        }
    }))
}
async fn verify_receipt(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &RunRecord,
    workflow: &StoredWorkflow,
    intent: &WorkflowInputAnswerIntent,
    receipt: &WorkflowInputReceipt,
) -> Result<()> {
    let q = workflow
        .inputs
        .get(&intent.request_id)
        .ok_or(Error::Forbidden)?;
    if q.receipt.as_ref() != Some(receipt)
        || q.answer_key.as_deref() != Some(&intent.submission.idempotency_key)
        || receipt.request_id != intent.request_id
        || receipt.fingerprint != intent.fingerprint
        || receipt.answer_sha256 != hash(intent.submission.input.answer.as_bytes())
        || receipt.confirmed_sequence <= 0
        || receipt.confirmed_sequence > parent.last_sequence
    {
        return Err(Error::Forbidden);
    }
    let row = sqlx::query("SELECT id,payload FROM events WHERE run_id=$1 AND sequence=$2")
        .bind(&parent.id)
        .bind(receipt.confirmed_sequence)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::Forbidden)?;
    let event: RunEvent = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if event.id != row.try_get::<String, _>("id").map_err(storage_error)?
        || event.run_id != parent.id
        || event.sequence != receipt.confirmed_sequence
        || event.kind != "workflow_input_answered"
        || event.payload.get("request_id").and_then(Value::as_str) != Some(&intent.request_id)
        || event.payload.get("node_id").and_then(Value::as_str) != Some(&q.node_id)
        || event.payload.get("model_turn_id").and_then(Value::as_str) != Some(&q.turn_id)
        || event.payload.get("receipt_id").and_then(Value::as_str) != Some(&receipt.receipt_id)
        || event.payload.get("answer_sha256").and_then(Value::as_str)
            != Some(&receipt.answer_sha256)
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
async fn read_intent(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &str,
    intent_id: &str,
    verify_accepted: bool,
) -> Result<WorkflowInputAnswerIntent> {
    let row=sqlx::query("SELECT project_id,request_id,owner,request_key,prepare_sha256,request_sha256,state,payload FROM workflow_input_answer_intents WHERE id=$1 AND run_id=$2 AND owner=$3").bind(intent_id).bind(run).bind(OWNER).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let intent: WorkflowInputAnswerIntent =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if intent.id != intent_id
        || intent.run_id != run
        || intent.owner != OWNER
        || intent.owner != row.try_get::<String, _>("owner").map_err(storage_error)?
        || intent.project_id
            != row
                .try_get::<String, _>("project_id")
                .map_err(storage_error)?
        || intent.request_id
            != row
                .try_get::<String, _>("request_id")
                .map_err(storage_error)?
        || intent.request_id != intent.submission.input.request_id
        || intent.fingerprint != intent.submission.fingerprint
        || intent.submission.idempotency_key
            != row
                .try_get::<String, _>("request_key")
                .map_err(storage_error)?
        || intent.request_sha256
            != row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
        || intent.request_sha256 != intent_hash(&intent)
        || prepare_hash(&intent)
            != row
                .try_get::<String, _>("prepare_sha256")
                .map_err(storage_error)?
        || state_name(&intent.state) != row.try_get::<String, _>("state").map_err(storage_error)?
        || intent.created_at < 0
        || intent.updated_at < intent.created_at
        || answer(&intent.submission.input.answer, true).is_err()
    {
        return Err(Error::Forbidden);
    }
    let (parent, workflow, _) =
        scope(tx, run, &intent.request_id, Some(&intent.fingerprint)).await?;
    if intent.project_id != parent.record.spec.project_id {
        return Err(Error::Forbidden);
    }
    let snapshot = read_snapshot(
        tx,
        run,
        &intent.request_id,
        &intent.fingerprint,
        &intent.draft_snapshot.snapshot_id,
    )
    .await?;
    if snapshot.conflict
        || snapshot.project_id != intent.project_id
        || snapshot.revision != intent.draft_snapshot.revision
        || snapshot.answer_sha256 != intent.draft_snapshot.answer_sha256
        || snapshot.answer != intent.submission.input.answer
    {
        return Err(Error::Forbidden);
    }
    match intent.state {
        WorkflowInputAnswerIntentState::Pending
            if intent.receipt.is_none() && intent.rejection.is_none() => {}
        WorkflowInputAnswerIntentState::Rejected
            if intent.receipt.is_none()
                && intent
                    .rejection
                    .as_ref()
                    .is_some_and(|r| rejection_code(&r.code)) => {}
        WorkflowInputAnswerIntentState::Accepted if intent.rejection.is_none() => {
            let receipt = intent.receipt.as_ref().ok_or(Error::Forbidden)?;
            if verify_accepted {
                verify_receipt(tx, &parent.record, &workflow, &intent, receipt).await?;
            }
        }
        _ => return Err(Error::Forbidden),
    }
    Ok(intent)
}
fn rejection_code(code: &str) -> bool {
    matches!(
        code,
        "forbidden" | "workflow_revoked" | "budget_exceeded" | "input_closed"
    )
}
impl Store {
    pub async fn workflow_input_draft(
        &self,
        run: &str,
        request: &str,
    ) -> Result<WorkflowInputDraftLookup> {
        let mut tx = self.snapshot_transaction().await?;
        let (parent, _, view) = scope(&mut tx, run, request, None).await?;
        let draft = read_head(&mut tx, &parent.record, &view)
            .await?
            .unwrap_or_else(|| WorkflowInputDraft {
                run_id: run.into(),
                project_id: parent.record.spec.project_id.clone(),
                request_id: request.into(),
                fingerprint: view.fingerprint.clone(),
                owner: OWNER.into(),
                revision: 0,
                answer: String::new(),
                answer_sha256: hash(b""),
                snapshot_id: None,
                updated_at: 0,
            });
        let ids=sqlx::query_scalar::<_,String>("SELECT id FROM workflow_input_draft_snapshots WHERE run_id=$1 AND request_id=$2 AND owner=$3 ORDER BY id").bind(run).bind(request).bind(OWNER).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if ids.len() > 128 {
            return Err(Error::Forbidden);
        }
        let mut snapshots = Vec::new();
        for id in ids {
            let s = read_snapshot(&mut tx, run, request, &view.fingerprint, &id).await?;
            if s.project_id != draft.project_id {
                return Err(Error::Forbidden);
            }
            snapshots.push(WorkflowInputDraftSnapshotSummary {
                id: s.id,
                base_revision: s.base_revision,
                revision: s.revision,
                answer_sha256: s.answer_sha256,
                conflict: s.conflict,
                created_at: s.created_at,
            });
        }
        let ids=sqlx::query_scalar::<_,String>("SELECT id FROM workflow_input_answer_intents WHERE run_id=$1 AND request_id=$2 AND owner=$3 ORDER BY id").bind(run).bind(request).bind(OWNER).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if ids.len() > 16 {
            return Err(Error::Forbidden);
        }
        let mut intents = Vec::new();
        for id in ids {
            let i = read_intent(&mut tx, run, &id, true).await?;
            intents.push(WorkflowInputAnswerIntentSummary {
                id: i.id,
                state: i.state,
                answer_sha256: i.draft_snapshot.answer_sha256,
                draft_revision: i.draft_snapshot.revision,
                created_at: i.created_at,
            });
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(WorkflowInputDraftLookup {
            draft,
            snapshots,
            intents,
        })
    }
    pub async fn workflow_input_draft_snapshot(
        &self,
        run: &str,
        request: &str,
        snapshot: &str,
    ) -> Result<WorkflowInputDraftSnapshot> {
        valid_text(snapshot, 128)?;
        let mut tx = self.snapshot_transaction().await?;
        let (parent, _, view) = scope(&mut tx, run, request, None).await?;
        let result = read_snapshot(&mut tx, run, request, &view.fingerprint, snapshot).await?;
        if result.project_id != parent.record.spec.project_id {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn save_workflow_input_draft(
        &self,
        run: &str,
        request: &str,
        input: &SaveWorkflowInputDraft,
    ) -> Result<WorkflowInputDraftSave> {
        valid_text(&input.idempotency_key, 128)?;
        fp(&input.fingerprint)?;
        answer(&input.answer, false)?;
        if input.base_revision < 0 || input.base_revision.checked_add(1).is_none() {
            return Err(Error::Validation("Invalid input draft basis".into()));
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (parent, workflow, view) =
            scope(&mut tx, run, request, Some(&input.fingerprint)).await?;
        if let Some(replay) = replay_save(&mut tx, &parent.record, &view, input).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(replay);
        }
        writable(&mut tx, &parent, &workflow, &view, false, now()).await?;
        let before = read_head(&mut tx, &parent.record, &view).await?;
        if before.is_none() && input.base_revision != 0 {
            return Err(Error::Conflict);
        }
        let mut snapshot = WorkflowInputDraftSnapshot {
            id: id(),
            run_id: run.into(),
            project_id: parent.record.spec.project_id.clone(),
            request_id: request.into(),
            fingerprint: input.fingerprint.clone(),
            owner: OWNER.into(),
            base_revision: input.base_revision,
            revision: input.base_revision + 1,
            answer: input.answer.clone(),
            answer_sha256: hash(input.answer.as_bytes()),
            conflict: false,
            created_at: now(),
        };
        let candidate = draft_from(&snapshot);
        let inserted=sqlx::query("INSERT INTO workflow_input_drafts(run_id,request_id,owner,project_id,revision,snapshot_id,payload) VALUES($1,$2,$3,$4,1,$5,$6) ON CONFLICT(run_id,request_id,owner) DO NOTHING").bind(run).bind(request).bind(OWNER).bind(&parent.record.spec.project_id).bind(&snapshot.id).bind(json_string(&candidate)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()==1;
        sqlx::query("UPDATE workflow_input_drafts SET revision=revision WHERE run_id=$1 AND request_id=$2 AND owner=$3").bind(run).bind(request).bind(OWNER).execute(&mut *tx).await.map_err(storage_error)?;
        if let Some(replay) = replay_save(&mut tx, &parent.record, &view, input).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(replay);
        }
        workflows::registry::lock_head(
            &mut tx,
            &parent.record.spec.project_id,
            &workflow.binding.workflow_id,
        )
        .await?;
        lock_run(&mut tx, run).await?;
        let (fresh_parent, fresh_workflow, fresh_view) =
            scope(&mut tx, run, request, Some(&input.fingerprint)).await?;
        // The run lock serializes this read with every clock writer. Draft
        // receipts do not mutate the run or advance its UTC high-water mark.
        let execution_at = self
            .workflow_clock
            .execution_now(&mut tx, &fresh_parent.record)
            .await?;
        writable(
            &mut tx,
            &fresh_parent,
            &fresh_workflow,
            &fresh_view,
            false,
            execution_at,
        )
        .await?;
        snapshot.created_at = execution_at;
        let current = if inserted {
            candidate
        } else {
            read_head(&mut tx, &parent.record, &view)
                .await?
                .ok_or(Error::Forbidden)?
        };
        snapshot.conflict = !inserted && current.revision != input.base_revision;
        snapshot.revision = if snapshot.conflict {
            current.revision
        } else if inserted {
            1
        } else {
            current
                .revision
                .checked_add(1)
                .ok_or(Error::BudgetExceeded)?
        };
        let count=sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM workflow_input_draft_snapshots WHERE run_id=$1 AND request_id=$2 AND owner=$3").bind(run).bind(request).bind(OWNER).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count >= 128 {
            return Err(Error::BudgetExceeded);
        }
        let payload = json_string(&snapshot)?;
        quota(
            &mut tx,
            &parent.record.spec.project_id,
            i64::from(inserted),
            1,
            payload.len() as i64,
            0,
        )
        .await?;
        sqlx::query("INSERT INTO workflow_input_draft_snapshots(id,run_id,request_id,owner,project_id,payload_bytes,payload) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(&snapshot.id).bind(run).bind(request).bind(OWNER).bind(&snapshot.project_id).bind(payload.len() as i64).bind(payload).execute(&mut *tx).await.map_err(storage_error)?;
        let draft = if snapshot.conflict {
            current
        } else {
            draft_from(&snapshot)
        };
        if !snapshot.conflict {
            sqlx::query("UPDATE workflow_input_drafts SET revision=$1,snapshot_id=$2,payload=$3 WHERE run_id=$4 AND request_id=$5 AND owner=$6").bind(draft.revision).bind(&draft.snapshot_id).bind(json_string(&draft)?).bind(run).bind(request).bind(OWNER).execute(&mut *tx).await.map_err(storage_error)?;
        }
        let receipt = SaveReceipt {
            head: draft.snapshot_id.clone().ok_or(Error::Forbidden)?,
            incoming: snapshot.id.clone(),
            conflict: snapshot.conflict,
        };
        sqlx::query("INSERT INTO workflow_input_draft_saves(run_id,request_id,owner,request_key,request_sha256,payload) VALUES($1,$2,$3,$4,$5,$6)").bind(run).bind(request).bind(OWNER).bind(&input.idempotency_key).bind(scope_hash(run,request,input)).bind(json_string(&receipt)?).execute(&mut *tx).await.map_err(storage_error)?;
        let result = if snapshot.conflict {
            WorkflowInputDraftSave::Conflict { draft, snapshot }
        } else {
            WorkflowInputDraftSave::Saved { draft, snapshot }
        };
        writable(
            &mut tx,
            &fresh_parent,
            &fresh_workflow,
            &fresh_view,
            false,
            execution_at,
        )
        .await?;
        // Sample again after quota and receipt SQL waits, immediately before
        // commit. Any denial rolls back the head, snapshot, quota and receipt.
        let final_at = self
            .workflow_clock
            .execution_now(&mut tx, &fresh_parent.record)
            .await?;
        if final_at < execution_at {
            return Err(Error::WorkflowClockRollback);
        }
        if final_at >= fresh_view.expires_at {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn prepare_workflow_input_answer(
        &self,
        run: &str,
        request: &str,
        input: &PrepareWorkflowInputAnswer,
    ) -> Result<WorkflowInputAnswerIntent> {
        valid_text(&input.idempotency_key, 128)?;
        fp(&input.fingerprint)?;
        if input.base_draft_revision < 1 {
            return Err(Error::Validation(
                "A confirmed answer draft is required".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (initial_parent, initial_workflow, _) =
            scope(&mut tx, run, request, Some(&input.fingerprint)).await?;
        let digest = scope_hash(run, request, input);
        if let Some(result) = prepare_replay(&mut tx, run, &input.idempotency_key, &digest).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(result);
        }
        if sqlx::query("UPDATE workflow_input_drafts SET revision=revision WHERE run_id=$1 AND request_id=$2 AND owner=$3").bind(run).bind(request).bind(OWNER).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1 { return Err(Error::NotFound); }
        if let Some(result) = prepare_replay(&mut tx, run, &input.idempotency_key, &digest).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(result);
        }
        workflows::registry::lock_head(
            &mut tx,
            &initial_parent.record.spec.project_id,
            &initial_workflow.binding.workflow_id,
        )
        .await?;
        lock_run(&mut tx, run).await?;
        let (parent, workflow, view) =
            scope(&mut tx, run, request, Some(&input.fingerprint)).await?;
        let execution_at = self
            .workflow_clock
            .execution_now(&mut tx, &parent.record)
            .await?;
        writable(&mut tx, &parent, &workflow, &view, true, execution_at).await?;
        let draft = read_head(&mut tx, &parent.record, &view)
            .await?
            .ok_or(Error::NotFound)?;
        if draft.revision != input.base_draft_revision {
            return Err(Error::Conflict);
        }
        answer(&draft.answer, true)?;
        let states=sqlx::query_scalar::<_,String>("SELECT state FROM workflow_input_answer_intents WHERE run_id=$1 AND request_id=$2 AND owner=$3").bind(run).bind(request).bind(OWNER).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if states.iter().any(|s| s == "pending") {
            return Err(Error::Conflict);
        }
        if states.len() >= 16 {
            return Err(Error::BudgetExceeded);
        }
        let mut intent = WorkflowInputAnswerIntent {
            id: id(),
            run_id: run.into(),
            project_id: parent.record.spec.project_id.clone(),
            request_id: request.into(),
            fingerprint: input.fingerprint.clone(),
            owner: OWNER.into(),
            request_sha256: String::new(),
            submission: SubmitWorkflowInput {
                input: SubmitInput {
                    request_id: request.into(),
                    answer: draft.answer,
                },
                fingerprint: input.fingerprint.clone(),
                idempotency_key: input.idempotency_key.clone(),
            },
            draft_snapshot: WorkflowInputDraftSnapshotRef {
                snapshot_id: draft.snapshot_id.ok_or(Error::Forbidden)?,
                revision: draft.revision,
                answer_sha256: draft.answer_sha256,
            },
            state: WorkflowInputAnswerIntentState::Pending,
            receipt: None,
            rejection: None,
            created_at: execution_at,
            updated_at: 0,
        };
        intent.updated_at = intent.created_at;
        intent.request_sha256 = intent_hash(&intent);
        quota(&mut tx, &intent.project_id, 0, 0, 0, 1).await?;
        let inserted=sqlx::query("INSERT INTO workflow_input_answer_intents(id,run_id,request_id,owner,project_id,request_key,prepare_sha256,request_sha256,state,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'pending',$9) ON CONFLICT(run_id,owner,request_key) DO NOTHING").bind(&intent.id).bind(run).bind(request).bind(OWNER).bind(&intent.project_id).bind(&input.idempotency_key).bind(&digest).bind(&intent.request_sha256).bind(json_string(&intent)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()==1;
        if !inserted {
            return Err(Error::Conflict);
        }
        let (fresh_parent, fresh_workflow, fresh_view) =
            scope(&mut tx, run, request, Some(&input.fingerprint)).await?;
        writable(
            &mut tx,
            &fresh_parent,
            &fresh_workflow,
            &fresh_view,
            true,
            execution_at,
        )
        .await?;
        let final_at = self
            .workflow_clock
            .execution_now(&mut tx, &fresh_parent.record)
            .await?;
        if final_at < execution_at {
            return Err(Error::WorkflowClockRollback);
        }
        if final_at >= fresh_view.expires_at {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(intent)
    }
    pub async fn workflow_input_answer_intent(
        &self,
        run: &str,
        intent: &str,
    ) -> Result<WorkflowInputAnswerIntent> {
        valid_text(run, 128)?;
        valid_text(intent, 128)?;
        let mut tx = self.snapshot_transaction().await?;
        let result = read_intent(&mut tx, run, intent, true).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn commit_workflow_input_answer_intent(
        &self,
        run: &str,
        intent_id: &str,
    ) -> Result<WorkflowInputAnswerIntent> {
        for attempt in 0..12 {
            let mut tx = self.snapshot_transaction().await?;
            let intent = read_intent(&mut tx, run, intent_id, true).await?;
            if intent.state != WorkflowInputAnswerIntentState::Pending {
                tx.commit().await.map_err(storage_error)?;
                return Ok(intent);
            }
            let (parent, before, _) =
                scope(&mut tx, run, &intent.request_id, Some(&intent.fingerprint)).await?;
            tx.commit().await.map_err(storage_error)?;
            if let Some(terminal) = self.close_known_input_intent(run, intent_id).await? {
                return Ok(terminal);
            }
            let lease = parent
                .lease
                .as_ref()
                .ok_or(Error::StaleLease)?
                .token
                .clone();
            let result = self
                .mutate(run, Some(&lease), |r| {
                    let (mut change, receipt) =
                        match workflows::inputs::answer_change(r, &before, &intent.submission)? {
                            workflows::inputs::PreparedWorkflowInputAnswer::Write(
                                change,
                                receipt,
                            ) => (*change, receipt),
                            workflows::inputs::PreparedWorkflowInputAnswer::Existing(receipt) => {
                                (Change::default(), receipt)
                            }
                        };
                    change.workflow_input_intent_write = Some(Box::new(WorkflowInputIntentWrite {
                        intent: intent.clone(),
                        receipt,
                    }));
                    Ok(change)
                })
                .await;
            match result {
                Ok(_) => return self.workflow_input_answer_intent(run, intent_id).await,
                Err(error) => {
                    // Concurrent acceptance wins even when our old lease/graph failed.
                    let current = self.workflow_input_answer_intent(run, intent_id).await?;
                    if current.state != WorkflowInputAnswerIntentState::Pending {
                        return Ok(current);
                    }
                    if matches!(error, Error::Conflict)
                        || matches!(&error,Error::Storage(e) if e.downcast_ref::<sqlx::Error>().is_some_and(retryable))
                    {
                        tokio::time::sleep(Duration::from_millis(10 * (attempt + 1))).await;
                        continue;
                    }
                    if let Some(terminal) = self.close_known_input_intent(run, intent_id).await? {
                        return Ok(terminal);
                    }
                    return Err(error);
                }
            }
        }
        Err(Error::Conflict)
    }
    async fn close_known_input_intent(
        &self,
        run: &str,
        intent_id: &str,
    ) -> Result<Option<WorkflowInputAnswerIntent>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        lock_intent(&mut tx, run, intent_id).await?;
        let mut intent = read_intent(&mut tx, run, intent_id, true).await?;
        if intent.state != WorkflowInputAnswerIntentState::Pending {
            tx.commit().await.map_err(storage_error)?;
            return Ok(Some(intent));
        }
        let (parent, workflow, _) =
            scope(&mut tx, run, &intent.request_id, Some(&intent.fingerprint)).await?;
        workflows::registry::lock_head(
            &mut tx,
            &parent.record.spec.project_id,
            &workflow.binding.workflow_id,
        )
        .await?;
        lock_run(&mut tx, run).await?;
        let (parent, workflow, view) =
            scope(&mut tx, run, &intent.request_id, Some(&intent.fingerprint)).await?;
        let entry = workflows::registry::read_entry(
            &mut tx,
            &parent.record.spec.project_id,
            &workflow.binding.workflow_id,
        )
        .await?;
        let stopped = workflow.plan.definition.failure
            == cowork_domain::workflows::WorkflowFailure::Stop
            && workflow.progress.nodes().values().any(|n| {
                matches!(
                    n.state,
                    cowork_domain::workflows::WorkflowNodeState::Failed { .. }
                        | cowork_domain::workflows::WorkflowNodeState::Blocked
                )
            });
        let code = if entry.revoked {
            Some("workflow_revoked")
        } else if matches!(
            parent.record.state,
            RunState::CancelRequested | RunState::Canceled
        ) || known_cancellation(&mut tx, &parent.record).await?
        {
            Some("input_closed")
        } else if parent.record.state == RunState::Expired
            || now() >= view.expires_at
            || time_budget_exhausted(&parent.record, now())
        {
            Some("budget_exceeded")
        } else if stopped
            || workflow
                .progress
                .input_checkpoints()
                .get(&intent.request_id)
                .is_some_and(|cursor| cursor.closed_reason.is_some())
            || matches!(parent.record.state, RunState::Failed | RunState::Completed)
        {
            Some("input_closed")
        } else {
            None
        };
        let Some(code) = code else {
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        };
        intent.state = WorkflowInputAnswerIntentState::Rejected;
        intent.rejection = Some(WorkflowInputAnswerRejection {
            code: code.into(),
            message: "The saved answer cannot be applied because this question is closed".into(),
        });
        // Rejection records a validated closure, never fresh execution
        // authority. Keep its evidence monotone even when UTC was rolled back.
        intent.updated_at = intent
            .created_at
            .max(intent.updated_at)
            .max(parent.record.updated_at)
            .max(now().max(0));
        if sqlx::query("UPDATE workflow_input_answer_intents SET state='rejected',payload=$1 WHERE id=$2 AND run_id=$3 AND owner=$4 AND state='pending'").bind(json_string(&intent)?).bind(intent_id).bind(run).bind(OWNER).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(intent))
    }
}
fn retryable(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|c| matches!(c.as_ref(), "5" | "517" | "40001" | "40P01"))
}
async fn prepare_replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &str,
    key: &str,
    digest: &str,
) -> Result<Option<WorkflowInputAnswerIntent>> {
    let row=sqlx::query("SELECT id,prepare_sha256 FROM workflow_input_answer_intents WHERE run_id=$1 AND owner=$2 AND request_key=$3").bind(run).bind(OWNER).bind(key).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row
        .try_get::<String, _>("prepare_sha256")
        .map_err(storage_error)?
        != digest
    {
        return Err(Error::Conflict);
    }
    Ok(Some(
        read_intent(
            tx,
            run,
            &row.try_get::<String, _>("id").map_err(storage_error)?,
            true,
        )
        .await?,
    ))
}
async fn lock_intent(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &str,
    intent: &str,
) -> Result<()> {
    if sqlx::query("UPDATE workflow_input_answer_intents SET state=state WHERE id=$1 AND run_id=$2 AND owner=$3").bind(intent).bind(run).bind(OWNER).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1 { return Err(Error::NotFound); }
    Ok(())
}
pub(crate) async fn verify_intent_write(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &RunRecord,
    change: &Change,
) -> Result<()> {
    let Some(write) = &change.workflow_input_intent_write else {
        return Ok(());
    };
    lock_intent(tx, &parent.id, &write.intent.id).await?;
    let before = read_intent(tx, &parent.id, &write.intent.id, false).await?;
    if before.state != WorkflowInputAnswerIntentState::Pending
        || before.request_sha256 != write.intent.request_sha256
        || canonical_hash(&before.submission) != canonical_hash(&write.intent.submission)
        || before.project_id != parent.spec.project_id
    {
        return Err(Error::Conflict);
    }
    if write.receipt.request_id != before.request_id
        || write.receipt.fingerprint != before.fingerprint
        || write.receipt.answer_sha256 != before.draft_snapshot.answer_sha256
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
pub(crate) async fn write_intent(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &RunRecord,
    change: &Change,
) -> Result<()> {
    let Some(write) = &change.workflow_input_intent_write else {
        return Ok(());
    };
    let workflow = workflows::runs::load(tx, parent).await?;
    verify_receipt(tx, parent, &workflow, &write.intent, &write.receipt).await?;
    let mut accepted = write.intent.clone();
    accepted.state = WorkflowInputAnswerIntentState::Accepted;
    accepted.receipt = Some(write.receipt.clone());
    accepted.rejection = None;
    accepted.updated_at = now();
    if sqlx::query("UPDATE workflow_input_answer_intents SET state='accepted',payload=$1 WHERE id=$2 AND run_id=$3 AND owner=$4 AND request_sha256=$5 AND state='pending'").bind(json_string(&accepted)?).bind(&accepted.id).bind(&parent.id).bind(OWNER).bind(&accepted.request_sha256).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
    Ok(())
}
pub(crate) async fn read_verified_input_export(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    runs: &[RunRecord],
) -> Result<Option<Value>> {
    let mut out = WorkflowInputAnswerExport {
        owner: OWNER.into(),
        drafts: Vec::new(),
        snapshots: Vec::new(),
        intents: Vec::new(),
    };
    for run in runs {
        let requests=sqlx::query_scalar::<_,String>("SELECT request_id FROM workflow_input_drafts WHERE run_id=$1 AND owner=$2 ORDER BY request_id").bind(&run.id).bind(OWNER).fetch_all(&mut **tx).await.map_err(storage_error)?;
        for request in requests {
            let (parent, _, view) = scope(tx, &run.id, &request, None).await?;
            if parent.record.spec.project_id != run.spec.project_id
                || parent.record.spec.order_id != run.spec.order_id
            {
                return Err(Error::Forbidden);
            }
            out.drafts.push(
                read_head(tx, &parent.record, &view)
                    .await?
                    .ok_or(Error::Forbidden)?,
            );
            let snapshots=sqlx::query_scalar::<_,String>("SELECT id FROM workflow_input_draft_snapshots WHERE run_id=$1 AND request_id=$2 AND owner=$3 ORDER BY id").bind(&run.id).bind(&request).bind(OWNER).fetch_all(&mut **tx).await.map_err(storage_error)?;
            if snapshots.len() > 128 {
                return Err(Error::Forbidden);
            }
            for s in snapshots {
                let s = read_snapshot(tx, &run.id, &request, &view.fingerprint, &s).await?;
                if s.project_id != run.spec.project_id {
                    return Err(Error::Forbidden);
                }
                out.snapshots.push(s);
            }
            let intents=sqlx::query_scalar::<_,String>("SELECT id FROM workflow_input_answer_intents WHERE run_id=$1 AND request_id=$2 AND owner=$3 ORDER BY id").bind(&run.id).bind(&request).bind(OWNER).fetch_all(&mut **tx).await.map_err(storage_error)?;
            if intents.len() > 16 {
                return Err(Error::Forbidden);
            }
            for i in intents {
                out.intents.push(read_intent(tx, &run.id, &i, true).await?);
            }
        }
    }
    if out.drafts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(serde_json::to_value(out).map_err(storage_error)?))
    }
}

async fn lock_run(tx: &mut sqlx::Transaction<'_, sqlx::Any>, run: &str) -> Result<()> {
    if sqlx::query("UPDATE runs SET revision=revision WHERE id=$1")
        .bind(run)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::NotFound);
    }
    Ok(())
}

#[cfg(test)]
#[path = "workflow_input_drafts/clock_tests.rs"]
mod clock_tests;
