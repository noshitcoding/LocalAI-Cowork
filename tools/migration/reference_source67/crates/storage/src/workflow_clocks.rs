//! Private durable UTC projection for native workflows. It never grants IO authority.
use super::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(crate) trait UtcSource: Send + Sync {
    fn now(&self) -> i64;
}
struct SystemUtc;
impl UtcSource for SystemUtc {
    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}
#[derive(Clone)]
pub(crate) struct WorkflowClock {
    source: Arc<dyn UtcSource>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkflowClockRecord {
    pub(crate) format_version: u32,
    pub(crate) run_id: String,
    pub(crate) project_id: String,
    pub(crate) order_id: String,
    pub(crate) revision: i64,
    pub(crate) last_run_revision: i64,
    pub(crate) highwater_ms: i64,
    pub(crate) created_at_ms: i64,
    pub(crate) spec_sha256: String,
    pub(crate) binding_sha256: String,
    pub(crate) plan_sha256: String,
    pub(crate) initial_workflow_sha256: String,
    pub(crate) creation_confirmed: bool,
    pub(crate) started_at_ms: Option<i64>,
}
#[derive(Clone, Debug)]
pub(crate) enum WorkflowClockRead {
    UnverifiedMissing,
    Verified(Box<WorkflowClockRecord>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowClockBlock {
    Unverified,
    Rollback,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowClockIntent {
    Execution,
    Cancellation,
    Recovery,
    QueueNonActivation(WorkflowClockBlock),
    HumanResumeNonActivation,
}
pub(crate) struct WorkflowClockStage {
    before: Box<StoredRun>,
    workflow: Box<StoredWorkflow>,
    clock: WorkflowClockRead,
    intent: WorkflowClockIntent,
    evidence_at_ms: i64,
    sampled_at_ms: i64,
}
impl WorkflowClockStage {
    pub(crate) fn evidence_at_ms(&self) -> i64 {
        self.evidence_at_ms
    }
    pub(crate) fn execution_floor_ms(&self) -> Option<i64> {
        match (&self.clock, self.intent) {
            (WorkflowClockRead::Verified(record), WorkflowClockIntent::Execution) => {
                Some(record.highwater_ms.max(self.sampled_at_ms))
            }
            _ => None,
        }
    }
    pub(crate) fn is_non_execution(&self) -> bool {
        self.intent != WorkflowClockIntent::Execution
    }
}
impl WorkflowClock {
    pub(crate) fn system() -> Self {
        Self {
            source: Arc::new(SystemUtc),
        }
    }
    #[cfg(test)]
    pub(crate) fn for_test(source: Arc<dyn UtcSource>) -> Self {
        Self { source }
    }
    pub(crate) fn now(&self) -> i64 {
        self.source.now()
    }
    fn sample(&self) -> Result<i64> {
        let sample = self.now();
        if sample < 0 {
            return Err(Error::WorkflowClockRollback);
        }
        Ok(sample)
    }
    pub(crate) async fn seed(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        run: &RunRecord,
        workflow: &StoredWorkflow,
    ) -> Result<()> {
        assert_current_run(tx, run).await?;
        workflow.validate(run)?;
        let current = workflows::runs::load(tx, run).await?;
        if run.state != RunState::Queued
            || run.revision != 1
            || run.last_sequence != 2
            || run.started_at.is_some()
            || run.steps != 0
            || !run.model_attempts.is_empty()
            || canonical_hash(&current) != canonical_hash(workflow)
            || workflow.progress.revision() != 0
            || workflow.progress.state() != cowork_domain::workflows::WorkflowState::Ready
            || !workflow.agents.is_empty()
            || !workflow.inputs.is_empty()
            || run.created_at < 0
            || run.updated_at != run.created_at
        {
            return Err(Error::Forbidden);
        }
        let actual: StoredRun = load_parent(tx, &run.id).await?;
        if actual.lease.is_some()
            || canonical_hash(&actual.checkpoint)
                != canonical_hash(&Checkpoint {
                    messages: actual.checkpoint.messages.clone(),
                    ..Default::default()
                })
        {
            return Err(Error::Forbidden);
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE run_id=$1")
            .bind(&run.id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
        if count != 0 {
            return Err(Error::Forbidden);
        }
        let sample = self.sample()?;
        if sample < run.created_at {
            return Err(Error::WorkflowClockRollback);
        }
        let record = WorkflowClockRecord {
            format_version: 1,
            run_id: run.id.clone(),
            project_id: run.spec.project_id.clone(),
            order_id: run.spec.order_id.clone(),
            revision: 1,
            last_run_revision: 1,
            highwater_ms: sample,
            created_at_ms: run.created_at,
            spec_sha256: canonical_hash(&run.spec),
            binding_sha256: canonical_hash(&run.spec.workflow),
            plan_sha256: workflow.plan.sha256.clone(),
            initial_workflow_sha256: canonical_hash(workflow),
            creation_confirmed: false,
            started_at_ms: None,
        };
        sqlx::query("INSERT INTO workflow_execution_clocks(run_id,revision,last_run_revision,highwater_ms,payload) VALUES($1,$2,$3,$4,$5)")
            .bind(&run.id).bind(record.revision).bind(record.last_run_revision)
            .bind(record.highwater_ms).bind(json_string(&record)?)
            .execute(&mut **tx).await.map_err(storage_error)?;
        Ok(())
    }
    pub(crate) async fn finish_creation(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        run: &RunRecord,
    ) -> Result<()> {
        assert_current_run(tx, run).await?;
        lock_clock(tx, &run.id).await?;
        let mut record = load_clock(tx, run, run.revision)
            .await?
            .ok_or(Error::WorkflowClockUnverified)?;
        let workflow = workflows::runs::load(tx, run).await?;
        validate_origin(&record, run, &workflow)?;
        if run.revision != 1
            || run.state != RunState::Queued
            || record.creation_confirmed
            || record.started_at_ms.is_some()
            || canonical_hash(&workflow) != record.initial_workflow_sha256
        {
            return Err(Error::Forbidden);
        }
        verify_creation_events(tx, run, &record).await?;
        let sample = self.sample()?;
        if sample < record.highwater_ms {
            return Err(Error::WorkflowClockRollback);
        }
        let old_revision = record.revision;
        record.revision = record.revision.checked_add(1).ok_or(Error::Forbidden)?;
        record.creation_confirmed = true;
        record.highwater_ms = sample;
        write_clock(tx, &record, old_revision, run.revision).await?;
        if self.sample()? < record.highwater_ms {
            return Err(Error::WorkflowClockRollback);
        }
        Ok(())
    }
    /// Last creation-only fence after outer journal SQL waits. Never call for a replay.
    pub(crate) async fn assert_creation_time(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        run: &RunRecord,
    ) -> Result<()> {
        assert_current_run(tx, run).await?;
        let mut record = load_clock(tx, run, run.revision)
            .await?
            .ok_or(Error::WorkflowClockUnverified)?;
        if !record.creation_confirmed
            || run.revision != 1
            || run.state != RunState::Queued
            || run.started_at.is_some()
            || record.started_at_ms.is_some()
        {
            return Err(Error::Forbidden);
        }
        verify_creation_events(tx, run, &record).await?;
        let sample = self.sample()?;
        if sample < record.highwater_ms {
            return Err(Error::WorkflowClockRollback);
        }
        let previous_revision = record.revision;
        record.revision = record.revision.checked_add(1).ok_or(Error::Forbidden)?;
        record.highwater_ms = sample;
        write_clock(tx, &record, previous_revision, run.revision).await?;
        if self.sample()? < record.highwater_ms {
            return Err(Error::WorkflowClockRollback);
        }
        Ok(())
    }
    pub(crate) async fn read(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        run: &RunRecord,
    ) -> Result<WorkflowClockRead> {
        assert_current_run(tx, run).await?;
        let Some(record) = load_clock(tx, run, run.revision).await? else {
            return Ok(WorkflowClockRead::UnverifiedMissing);
        };
        let workflow = workflows::runs::load(tx, run).await?;
        validate_origin(&record, run, &workflow)?;
        if !record.creation_confirmed {
            return Err(Error::Forbidden);
        }
        if record.started_at_ms != run.started_at {
            return Err(Error::Forbidden);
        }
        verify_creation_events(tx, run, &record).await?;
        Ok(WorkflowClockRead::Verified(Box::new(record)))
    }
    pub(crate) async fn execution_now(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        run: &RunRecord,
    ) -> Result<i64> {
        let record = match self.read(tx, run).await? {
            WorkflowClockRead::UnverifiedMissing => return Err(Error::WorkflowClockUnverified),
            WorkflowClockRead::Verified(record) => record,
        };
        // Corrupt origin/row metadata is never hidden by a sticky public denial.
        sticky_denial(run)?;
        let sample = self.sample()?;
        if sample < record.highwater_ms {
            return Err(Error::WorkflowClockRollback);
        }
        Ok(sample)
    }
    pub(crate) async fn stage(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        before: &StoredRun,
        intent: WorkflowClockIntent,
    ) -> Result<WorkflowClockStage> {
        let clock = self.read(tx, &before.record).await?;
        let workflow = workflows::runs::load(tx, &before.record).await?;
        let sample = self.now();
        let highwater = match &clock {
            WorkflowClockRead::Verified(record) => Some(record.highwater_ms),
            WorkflowClockRead::UnverifiedMissing => None,
        };
        match intent {
            WorkflowClockIntent::Execution => {
                sticky_denial(&before.record)?;
                let highwater = highwater.ok_or(Error::WorkflowClockUnverified)?;
                if sample < highwater {
                    return Err(Error::WorkflowClockRollback);
                }
            }
            WorkflowClockIntent::QueueNonActivation(reason) => {
                if !matches!(
                    before.record.state,
                    RunState::Queued | RunState::WaitingForExecutor
                ) || before.lease.is_some()
                {
                    return Err(Error::Forbidden);
                }
                match reason {
                    WorkflowClockBlock::Unverified if highwater.is_none() => {}
                    WorkflowClockBlock::Rollback if highwater.is_some_and(|v| sample < v) => {}
                    _ => return Err(Error::Conflict),
                }
            }
            WorkflowClockIntent::Cancellation | WorkflowClockIntent::Recovery => {}
            WorkflowClockIntent::HumanResumeNonActivation => {
                if !matches!(
                    before.record.state,
                    RunState::Queued
                        | RunState::WaitingForExecutor
                        | RunState::Canceled
                        | RunState::Expired
                ) || before.lease.is_some()
                    || workflow
                        .progress
                        .human_continuations()
                        .last()
                        .is_none_or(|c| {
                            !c.execution_claims.is_empty() || c.blocked_reason.is_some()
                        })
                {
                    return Err(Error::Forbidden);
                }
            }
        }
        let domain_at_ms = workflow
            .progress
            .last_observed_at()
            .unwrap_or(0)
            .checked_mul(1000)
            .ok_or(Error::Forbidden)?;
        let evidence_at_ms = before
            .record
            .updated_at
            .max(highwater.unwrap_or(0))
            .max(domain_at_ms)
            .max(sample.max(0));
        Ok(WorkflowClockStage {
            before: Box::new(before.clone()),
            workflow: Box::new(workflow),
            clock,
            intent,
            evidence_at_ms,
            sampled_at_ms: sample,
        })
    }
    pub(crate) async fn finish(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        stage: &WorkflowClockStage,
        after: &StoredRun,
        change: &Change,
        execution_floor_ms: i64,
    ) -> Result<Option<i64>> {
        let before = &stage.before;
        if after.record.revision
            != before
                .record
                .revision
                .checked_add(1)
                .ok_or(Error::Forbidden)?
            || after.record.id != before.record.id
            || canonical_hash(&after.record.spec) != canonical_hash(&before.record.spec)
            || after.record.created_at != before.record.created_at
        {
            return Err(Error::Forbidden);
        }
        let actual = load_parent(tx, &after.record.id).await?;
        if canonical_hash(&actual) != canonical_hash(after) {
            return Err(Error::Conflict);
        }
        if after.record.updated_at < before.record.updated_at
            || stage.is_non_execution() && after.record.updated_at < stage.evidence_at_ms
        {
            return Err(Error::Forbidden);
        }
        verify_delta_events(tx, stage, after, change).await?;
        if stage.is_non_execution() {
            verify_closure(tx, stage, after, change).await?;
        }
        // No Registry/Run/Quota/Folder lock may be acquired after this clock lock.
        let locked = lock_clock_optional(tx, &after.record.id).await?;
        match &stage.clock {
            WorkflowClockRead::UnverifiedMissing => {
                if locked || !stage.is_non_execution() {
                    return Err(Error::WorkflowClockUnverified);
                }
                return Ok(None); // A pure legacy closure never creates a clock origin.
            }
            WorkflowClockRead::Verified(old) => {
                if !locked {
                    return Err(Error::Forbidden);
                }
                let mut record = load_clock(tx, &before.record, before.record.revision)
                    .await?
                    .ok_or(Error::Forbidden)?;
                if canonical_hash(&record) != canonical_hash(old) {
                    return Err(Error::Conflict);
                }
                let next_workflow = workflows::runs::load(tx, &after.record).await?;
                validate_origin(&record, &before.record, &next_workflow)?;
                verify_creation_events(tx, &before.record, &record).await?;
                let sample = self.now();
                let minimum = stage
                    .execution_floor_ms()
                    .unwrap_or(record.highwater_ms)
                    .max(record.highwater_ms)
                    .max(after.record.updated_at)
                    .max(execution_floor_ms);
                if !stage.is_non_execution() && sample < minimum {
                    return Err(Error::WorkflowClockRollback);
                }
                if stage.intent
                    == WorkflowClockIntent::QueueNonActivation(WorkflowClockBlock::Rollback)
                    && sample >= record.highwater_ms
                {
                    return Err(Error::Conflict);
                }
                if stage.intent == WorkflowClockIntent::HumanResumeNonActivation
                    && change.workflow_resume_block.as_deref() == Some("workflow_clock_rollback")
                    && sample >= record.highwater_ms
                {
                    return Err(Error::Conflict);
                }
                if after.record.updated_at < record.highwater_ms
                    || after.record.updated_at < before.record.updated_at
                    || stage.is_non_execution() && after.record.updated_at < stage.evidence_at_ms
                {
                    return Err(Error::Forbidden);
                }
                if record.started_at_ms != before.record.started_at {
                    return Err(Error::Forbidden);
                }
                if before.record.started_at != after.record.started_at {
                    if stage.is_non_execution()
                        || before.record.started_at.is_some()
                        || !matches!(
                            before.record.state,
                            RunState::Queued | RunState::WaitingForExecutor
                        )
                        || after.record.state != RunState::Running
                        || after.record.started_at.is_none_or(|at| {
                            at < stage.execution_floor_ms().unwrap_or(record.highwater_ms)
                                || at > after.record.updated_at
                        })
                    {
                        return Err(Error::Forbidden);
                    }
                    record.started_at_ms = after.record.started_at;
                }
                let previous_revision = record.revision;
                record.revision = record.revision.checked_add(1).ok_or(Error::Forbidden)?;
                record.last_run_revision = after.record.revision;
                record.highwater_ms = record.highwater_ms.max(stage.sampled_at_ms).max(sample);
                write_clock(tx, &record, previous_revision, before.record.revision).await?;
                if !stage.is_non_execution() {
                    let final_sample = self.now();
                    if final_sample < record.highwater_ms.max(minimum) {
                        return Err(Error::WorkflowClockRollback);
                    }
                    return Ok(Some(final_sample.max(record.highwater_ms)));
                }
            }
        }
        Ok(None)
    }
}
async fn load_parent(tx: &mut sqlx::Transaction<'_, sqlx::Any>, run: &str) -> Result<StoredRun> {
    let row =
        sqlx::query("SELECT id,order_id,revision,state,updated_at,payload FROM runs WHERE id=$1")
            .bind(run)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
    let parent: StoredRun =
        serde_json::from_str(&row.try_get::<String, _>("payload").map_err(storage_error)?)
            .map_err(|_| Error::Forbidden)?;
    if parent.record.id != row.try_get::<String, _>("id").map_err(storage_error)?
        || parent.record.spec.order_id
            != row
                .try_get::<String, _>("order_id")
                .map_err(storage_error)?
        || parent.record.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || state_name(&parent.record.state)
            != row.try_get::<String, _>("state").map_err(storage_error)?
        || parent.record.updated_at != row.try_get::<i64, _>("updated_at").map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(parent)
}
async fn assert_current_run(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &RunRecord,
) -> Result<()> {
    if run.spec.workflow.is_none() {
        return Err(Error::Forbidden);
    }
    let parent = load_parent(tx, &run.id).await?;
    if canonical_hash(&parent.record) != canonical_hash(run) {
        return Err(Error::Conflict);
    }
    Ok(())
}
async fn load_clock(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &RunRecord,
    expected_run_revision: i64,
) -> Result<Option<WorkflowClockRecord>> {
    let Some(row) = sqlx::query("SELECT revision,last_run_revision,highwater_ms,payload FROM workflow_execution_clocks WHERE run_id=$1")
        .bind(&run.id).fetch_optional(&mut **tx).await.map_err(storage_error)?
    else { return Ok(None); };
    let record: WorkflowClockRecord =
        serde_json::from_str(&row.try_get::<String, _>("payload").map_err(storage_error)?)
            .map_err(|_| Error::Forbidden)?;
    if record.format_version != 1
        || record.run_id != run.id
        || record.project_id != run.spec.project_id
        || record.order_id != run.spec.order_id
        || record.created_at_ms != run.created_at
        || record.spec_sha256 != canonical_hash(&run.spec)
        || record.binding_sha256 != canonical_hash(&run.spec.workflow)
        || record.revision < 1
        || record.last_run_revision != expected_run_revision
        || record.highwater_ms < record.created_at_ms
        || record.created_at_ms < 0
        || record.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || record.last_run_revision
            != row
                .try_get::<i64, _>("last_run_revision")
                .map_err(storage_error)?
        || record.highwater_ms
            != row
                .try_get::<i64, _>("highwater_ms")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(Some(record))
}
pub(crate) async fn read_highwater_for_resume(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &RunRecord,
) -> Result<Option<i64>> {
    Ok(load_clock(tx, run, run.revision)
        .await?
        .map(|r| r.highwater_ms))
}
fn validate_origin(
    record: &WorkflowClockRecord,
    run: &RunRecord,
    workflow: &StoredWorkflow,
) -> Result<()> {
    if run.spec.workflow.as_ref() != Some(&workflow.binding)
        || record.plan_sha256 != workflow.plan.sha256
        || record.initial_workflow_sha256
            != canonical_hash(&StoredWorkflow {
                format_version: 1,
                run_id: run.id.clone(),
                project_id: run.spec.project_id.clone(),
                binding: workflow.binding.clone(),
                plan: workflow.plan.clone(),
                progress: cowork_domain::workflows::WorkflowProgress::new(&workflow.plan)?,
                agents: Default::default(),
                inputs: Default::default(),
            })
        || record
            .started_at_ms
            .is_some_and(|at| at < record.created_at_ms || at > record.highwater_ms)
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
async fn verify_creation_events(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &RunRecord,
    record: &WorkflowClockRecord,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id,sequence,payload FROM events WHERE run_id=$1 AND sequence<=2 ORDER BY sequence",
    )
    .bind(&run.id)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    if rows.len() != 2 || run.last_sequence < 2 {
        return Err(Error::Forbidden);
    }
    for (index, row) in rows.iter().enumerate() {
        let event: RunEvent =
            serde_json::from_str(&row.try_get::<String, _>("payload").map_err(storage_error)?)
                .map_err(|_| Error::Forbidden)?;
        let sequence = index as i64 + 1;
        if event.id != row.try_get::<String, _>("id").map_err(storage_error)?
            || event.run_id != run.id
            || event.sequence != sequence
            || row.try_get::<i64, _>("sequence").map_err(storage_error)? != sequence
            || event.created_at != record.created_at_ms
            || event.kind
                != if index == 0 {
                    "run_queued"
                } else {
                    "workflow_initialized"
                }
            || event.payload
                != if index == 0 {
                    json!({"state":"queued"})
                } else {
                    json!({"binding":run.spec.workflow})
                }
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}
async fn lock_clock_optional(tx: &mut sqlx::Transaction<'_, sqlx::Any>, run: &str) -> Result<bool> {
    Ok(
        sqlx::query("UPDATE workflow_execution_clocks SET revision=revision WHERE run_id=$1")
            .bind(run)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
            == 1,
    )
}
async fn lock_clock(tx: &mut sqlx::Transaction<'_, sqlx::Any>, run: &str) -> Result<()> {
    if !lock_clock_optional(tx, run).await? {
        return Err(Error::WorkflowClockUnverified);
    }
    Ok(())
}
async fn write_clock(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    record: &WorkflowClockRecord,
    previous_revision: i64,
    previous_run_revision: i64,
) -> Result<()> {
    if sqlx::query("UPDATE workflow_execution_clocks SET revision=$1,last_run_revision=$2,highwater_ms=$3,payload=$4 WHERE run_id=$5 AND revision=$6 AND last_run_revision=$7")
        .bind(record.revision).bind(record.last_run_revision).bind(record.highwater_ms)
        .bind(json_string(record)?).bind(&record.run_id).bind(previous_revision)
        .bind(previous_run_revision).execute(&mut **tx).await.map_err(storage_error)?
        .rows_affected() != 1
    { return Err(Error::Conflict); }
    Ok(())
}
async fn verify_closure(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    stage: &WorkflowClockStage,
    after: &StoredRun,
    change: &Change,
) -> Result<()> {
    let before = &stage.before;
    if change.claim_folders
        || !change.messages.is_empty()
        || !change.artifacts.is_empty()
        || !change.file_changes.is_empty()
        || !change.file_operations.is_empty()
        || change.workspace_snapshot.is_some()
        || change.environment_observation.is_some()
        || change.process_write.is_some()
        || change.mcp_write.is_some()
        || change.mcp_connection_write.is_some()
        || change.workflow_tool_gate.is_some()
        || change.workflow_input_intent_write.is_some()
        || change.workflow_model_dispatch_gate.is_some()
        || change.workflow_finish
        || canonical_hash(&before.checkpoint) != canonical_hash(&after.checkpoint)
        || after.record.steps != before.record.steps
        || after.record.started_at != before.record.started_at
        || after.lease.as_ref().is_some_and(|lease| {
            before
                .lease
                .as_ref()
                .is_none_or(|old| old.token != lease.token || old.until != lease.until)
        })
        || !conservative_usage(&before.record.usage, &after.record.usage)
        || !conservative_models(
            &before.record.model_attempts,
            &after.record.model_attempts,
            after.record.updated_at,
        )
    {
        return Err(Error::Forbidden);
    }
    let next = workflows::runs::load(tx, &after.record).await?;
    if canonical_hash(&next.plan) != canonical_hash(&stage.workflow.plan)
        || canonical_hash(&next.agents) != canonical_hash(&stage.workflow.agents)
        || canonical_hash(&next.inputs) != canonical_hash(&stage.workflow.inputs)
        || next.binding != stage.workflow.binding
    {
        return Err(Error::Forbidden);
    }
    if stage.intent == WorkflowClockIntent::HumanResumeNonActivation {
        let reason = change
            .workflow_resume_block
            .as_deref()
            .ok_or(Error::Forbidden)?;
        let c = stage
            .workflow
            .progress
            .human_continuations()
            .last()
            .ok_or(Error::Forbidden)?;
        crate::workflow_operations::verify_resume_accepted(tx, &before.record, &stage.workflow, c)
            .await?;
        if before.lease.is_some()
            || after.lease.is_some()
            || !(after.record.state == RunState::Interrupted
                && matches!(
                    before.record.state,
                    RunState::Queued | RunState::WaitingForExecutor
                )
                || matches!(before.record.state, RunState::Canceled | RunState::Expired)
                    && before.record.state == after.record.state)
            || change.events
                != vec![(
                    "workflow_input_resume_blocked".into(),
                    json!({"resume_id":c.id,"generation":c.generation,"reason":reason}),
                )]
            || (after.record.state == RunState::Interrupted
                && after.record.error_code.as_deref() != Some(reason))
            || (terminal(&before.record.state)
                && after.record.error_code != before.record.error_code)
        {
            return Err(Error::Forbidden);
        }
        let highwater = match &stage.clock {
            WorkflowClockRead::Verified(c) => Some(c.highwater_ms),
            _ => None,
        };
        let actual = workflows::continuations::block_reason(
            tx,
            before,
            &stage.workflow,
            stage.sampled_at_ms,
            highwater,
        )
        .await?
        .ok_or(Error::Conflict)?;
        if actual != reason {
            return Err(Error::Conflict);
        }
        let mut expected = stage.workflow.progress.clone();
        expected.block_human(
            &stage.workflow.plan,
            expected.revision(),
            &c.id,
            c.generation,
            reason,
            next.progress.last_observed_at().ok_or(Error::Forbidden)?,
        )?;
        if canonical_hash(&expected) != canonical_hash(&next.progress) {
            return Err(Error::Forbidden);
        }
        return Ok(());
    }
    if after.record.state == RunState::Canceled
        && (after.checkpoint.in_flight.is_some()
            || next.progress.attempts().values().any(|a| {
                (a.outcome.is_none() && !next.progress.input_checkpoints().contains_key(&a.id))
                    || a.outcome == Some(cowork_domain::workflows::WorkflowOutcome::Uncertain)
            }))
    {
        return Err(Error::UncertainEffect);
    }
    match stage.intent {
        WorkflowClockIntent::Cancellation => {
            let known = workflow_input_drafts::known_cancellation(tx, &before.record).await?;
            let request = after.record.state == RunState::CancelRequested
                && matches!(
                    before.record.state,
                    RunState::Running | RunState::WaitingForSnapshot
                );
            let immediate = after.record.state == RunState::Canceled
                && !matches!(
                    before.record.state,
                    RunState::Running | RunState::CancelRequested | RunState::WaitingForSnapshot
                );
            let cleanup = known
                && matches!(
                    after.record.state,
                    RunState::Canceled | RunState::Interrupted
                );
            let unchanged_terminal =
                terminal(&before.record.state) && before.record.state == after.record.state;
            if !(request || immediate || cleanup || unchanged_terminal) {
                return Err(Error::Forbidden);
            }
        }
        WorkflowClockIntent::Recovery => {
            if after.record.state != RunState::Interrupted
                || after.lease.is_some()
                || !matches!(
                    before.record.state,
                    RunState::Running
                        | RunState::CancelRequested
                        | RunState::WaitingForSnapshot
                        | RunState::Interrupted
                )
                || before.lease.is_none() && before.record.state != RunState::Interrupted
            {
                return Err(Error::Forbidden);
            }
        }
        WorkflowClockIntent::QueueNonActivation(reason) => {
            if before.record.state != after.record.state
                || !matches!(
                    after.record.state,
                    RunState::Queued | RunState::WaitingForExecutor
                )
                || before.lease.is_some()
                || after.lease.is_some()
            {
                return Err(Error::Forbidden);
            }
            match (&stage.clock, reason) {
                (WorkflowClockRead::UnverifiedMissing, WorkflowClockBlock::Unverified) => {}
                (WorkflowClockRead::Verified(record), WorkflowClockBlock::Rollback)
                    if self_sample_below(stage, record) => {}
                _ => return Err(Error::Conflict),
            }
        }
        WorkflowClockIntent::Execution => return Err(Error::Forbidden),
        WorkflowClockIntent::HumanResumeNonActivation => return Err(Error::Forbidden),
    }
    if canonical_hash(&next.progress) != canonical_hash(&stage.workflow.progress) {
        if after.record.state != RunState::Interrupted {
            return Err(Error::Forbidden);
        }
        let mut expected = stage.workflow.progress.clone();
        expected.recover(
            &stage.workflow.plan,
            expected.revision(),
            next.progress.last_observed_at().ok_or(Error::Forbidden)?,
        )?;
        if canonical_hash(&expected) != canonical_hash(&next.progress) {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}
fn sticky_denial(run: &RunRecord) -> Result<()> {
    match run.error_code.as_deref() {
        Some("workflow_clock_unverified") => Err(Error::WorkflowClockUnverified),
        Some("workflow_clock_rollback") => Err(Error::WorkflowClockRollback),
        _ => Ok(()),
    }
}
async fn verify_delta_events(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    stage: &WorkflowClockStage,
    after: &StoredRun,
    change: &Change,
) -> Result<()> {
    let before = &stage.before.record;
    let count = after
        .record
        .last_sequence
        .checked_sub(before.last_sequence)
        .ok_or(Error::Forbidden)?;
    if count < 0 || count as usize != change.events.len() {
        return Err(Error::Forbidden);
    }
    let rows = sqlx::query("SELECT id,sequence,payload FROM events WHERE run_id=$1 AND sequence>$2 AND sequence<=$3 ORDER BY sequence")
        .bind(&before.id).bind(before.last_sequence).bind(after.record.last_sequence)
        .fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() != change.events.len() {
        return Err(Error::Forbidden);
    }
    for (index, (row, (kind, payload))) in rows.iter().zip(&change.events).enumerate() {
        let event: RunEvent =
            serde_json::from_str(&row.try_get::<String, _>("payload").map_err(storage_error)?)
                .map_err(|_| Error::Forbidden)?;
        let sequence = before.last_sequence + index as i64 + 1;
        if event.id != row.try_get::<String, _>("id").map_err(storage_error)?
            || event.run_id != before.id
            || event.sequence != sequence
            || row.try_get::<i64, _>("sequence").map_err(storage_error)? != sequence
            || event.created_at != after.record.updated_at
            || &event.kind != kind
            || &event.payload != payload
        {
            return Err(Error::Forbidden);
        }
        if stage.is_non_execution()
            && !matches!(
                kind.as_str(),
                "cancel_requested"
                    | "run_canceled"
                    | "run_interrupted"
                    | "workflow_interrupted"
                    | "workflow_cancel_interrupted"
                    | "workflow_checkpoint_recovered"
                    | "model_usage_unconfirmed"
                    | "workflow_activation_blocked"
                    | "workflow_input_resume_blocked"
            )
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}
fn self_sample_below(stage: &WorkflowClockStage, record: &WorkflowClockRecord) -> bool {
    // The reason was verified against an actual fresh sample in stage().
    stage.intent == WorkflowClockIntent::QueueNonActivation(WorkflowClockBlock::Rollback)
        && record.creation_confirmed
        && stage.sampled_at_ms < record.highwater_ms
}
fn conservative_usage(before: &Usage, after: &Usage) -> bool {
    let mut normalized = after.clone();
    normalized.incomplete = before.incomplete;
    canonical_hash(&normalized) == canonical_hash(before)
        && (!before.incomplete || after.incomplete)
}
fn conservative_models(before: &[ModelAttempt], after: &[ModelAttempt], evidence_at: i64) -> bool {
    before.len() == after.len()
        && before.iter().zip(after).all(|(old, new)| {
            if canonical_hash(old) == canonical_hash(new) {
                return true;
            }
            let mut normalized = new.clone();
            normalized.state = old.state.clone();
            normalized.finished_at = old.finished_at;
            old.state == ModelAttemptState::InFlight
                && new.state == ModelAttemptState::Interrupted
                // Domain Recover interrupts ownership without confirming an
                // unresolved model outcome or inventing its completion time.
                && (new.finished_at.is_none() && old.finished_at.is_none()
                    || new
                        .finished_at
                        .is_some_and(|at| at >= old.started_at && at <= evidence_at))
                && canonical_hash(&normalized) == canonical_hash(old)
        })
}
#[cfg(test)]
#[path = "workflow_clocks/tests.rs"]
mod tests;
