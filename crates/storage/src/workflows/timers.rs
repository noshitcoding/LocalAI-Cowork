//! Timer index is a projection. Every wake revalidates the original graph,
//! clock, current parent and revocation inside the parent transaction.
use super::*;

impl Store {
    pub(crate) async fn start_workflow_timer(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
    ) -> Result<WorkflowCommandResult> {
        self.mutate(run, Some(lease), |parent| {
            if parent.record.spec.project_id != project || parent.record.state != RunState::Running
            {
                return Err(Error::Conflict);
            }
            let mut change =
                Change::event("workflow_timer_scheduled", json!({"node_id":claim.node_id}));
            change.workflow_timer_start = Some((expected, claim.clone(), id.into()));
            Ok(change)
        })
        .await?;
        Ok(WorkflowCommandResult::Confirmed)
    }
    pub async fn advance_workflow_timers(
        &self,
        project: &str,
        run: &str,
        lease: &str,
    ) -> Result<bool> {
        let before = self.stored_workflow(project, run).await?;
        if before
            .progress
            .next_wake_at()
            .is_none_or(|at| at > self.workflow_clock.now().div_euclid(1000))
        {
            return Ok(false);
        }
        let result = self
            .mutate(run, Some(lease), |parent| {
                if parent.record.spec.project_id != project
                    || parent.record.state != RunState::Running
                {
                    return Err(Error::Conflict);
                }
                let mut change = Change::event("workflow_timers_advanced", json!({}));
                change.workflow_timers_advance = true;
                Ok(change)
            })
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(Error::Conflict) => Ok(false),
            Err(error) => Err(error),
        }
    }
    pub async fn park_workflow_timer(
        &self,
        project: &str,
        run: &str,
        lease: &str,
    ) -> Result<StoredRun> {
        self.mutate(run, Some(lease), |parent| {
            if parent.record.spec.project_id != project || parent.record.state != RunState::Running
            {
                return Err(Error::Conflict);
            }
            parent.transition(RunState::WaitingForTimer)?;
            parent.lease = None;
            let mut change = Change::event(
                "workflow_waiting_for_timer",
                json!({"state":"waiting_for_timer"}),
            );
            change.workflow_timer_park = true;
            Ok(change)
        })
        .await
    }
    pub async fn wake_workflow_timers(&self) -> Result<usize> {
        let candidates=sqlx::query_scalar::<_,String>("SELECT w.run_id FROM workflow_timer_wakes w JOIN runs r ON r.id=w.run_id WHERE r.state='waiting_for_timer' AND w.wake_at<=$1 ORDER BY w.wake_at,w.run_id LIMIT 100")
            .bind(self.workflow_clock.now().div_euclid(1000)).fetch_all(&self.pool).await.map_err(storage_error)?;
        let mut count = 0;
        for run in candidates {
            let result = self
                .mutate(&run, None, |parent| {
                    if parent.record.state != RunState::WaitingForTimer || parent.lease.is_some() {
                        return Err(Error::Conflict);
                    }
                    parent.transition(RunState::Queued)?;
                    let mut change =
                        Change::event("workflow_timer_queued", json!({"state":"queued"}));
                    change.workflow_timer_wake = true;
                    Ok(change)
                })
                .await;
            match result {
                Ok(_) => count += 1,
                Err(Error::Conflict) => {}
                Err(Error::WorkflowClockUnverified) => {
                    self.block_workflow_activation(
                        &run,
                        crate::workflow_clocks::WorkflowClockBlock::Unverified,
                    )
                    .await?
                }
                Err(Error::WorkflowClockRollback) => {
                    self.block_workflow_activation(
                        &run,
                        crate::workflow_clocks::WorkflowClockBlock::Rollback,
                    )
                    .await?
                }
                Err(Error::Forbidden | Error::Validation(_)) => {
                    // A damaged derived index never authorizes a wake and must
                    // not prevent independent queued work from being claimed.
                }
                Err(error) => return Err(error),
            }
        }
        Ok(count)
    }
}
pub(super) async fn validate_projection(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    workflow: &StoredWorkflow,
) -> Result<()> {
    let row=sqlx::query("SELECT progress_revision,progress_sha256,wake_at FROM workflow_timer_wakes WHERE run_id=$1")
        .bind(&workflow.run_id).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    if row
        .try_get::<i64, _>("progress_revision")
        .map_err(storage_error)?
        != workflow.progress.revision() as i64
        || row
            .try_get::<String, _>("progress_sha256")
            .map_err(storage_error)?
            != canonical_hash(&workflow.progress)
        || Some(row.try_get::<i64, _>("wake_at").map_err(storage_error)?)
            != workflow.progress.next_wake_at()
        || !workflow.progress.can_park_timer(&workflow.plan)?
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
fn write_progress(
    change: &mut Change,
    current: &StoredWorkflow,
    next: StoredWorkflow,
    new_action: bool,
) {
    change.workflow_write = Some(WorkflowWrite {
        new_action,
        expected_revision: current.progress.revision(),
        expected_sha256: canonical_hash(&current.progress),
        next,
    });
}
pub(super) async fn prepare(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &mut StoredRun,
    change: &mut Change,
    old_state: &RunState,
    current: &StoredWorkflow,
    conservative_at: Option<i64>,
    clock: &crate::workflow_clocks::WorkflowClock,
) -> Result<()> {
    let flags = usize::from(change.workflow_timer_start.is_some())
        + usize::from(change.workflow_timers_advance)
        + usize::from(change.workflow_timer_park)
        + usize::from(change.workflow_timer_wake);
    if flags > 1 || flags > 0 && change.workflow_write.is_some() {
        return Err(Error::Forbidden);
    }
    if *old_state == RunState::WaitingForTimer {
        if parent.record.state != RunState::Canceled {
            validate_projection(tx, current).await?;
        }
        if parent.record.state == RunState::Queued && !change.workflow_timer_wake {
            return Err(Error::Forbidden);
        }
    }
    if parent.record.state == RunState::WaitingForTimer
        && *old_state != RunState::WaitingForTimer
        && !change.workflow_timer_park
    {
        return Err(Error::Forbidden);
    }
    let close_reason = if *old_state == RunState::WaitingForTimer
        && matches!(parent.record.state, RunState::Expired | RunState::Failed)
    {
        Some("budget_exceeded")
    } else {
        None
    };
    if flags == 0 && close_reason.is_none() {
        return Ok(());
    }
    let at_ms = match conservative_at {
        Some(at) => at.checked_mul(1000).ok_or(Error::Forbidden)?,
        None => {
            let payload = sqlx::query_scalar::<_, String>("SELECT payload FROM runs WHERE id=$1")
                .bind(&parent.record.id)
                .fetch_one(&mut **tx)
                .await
                .map_err(storage_error)?;
            let original: StoredRun = parse(&payload)?;
            clock.execution_now(tx, &original.record).await?
        }
    };
    let at = at_ms.div_euclid(1000);
    if let Some((expected, claim, id)) = &change.workflow_timer_start {
        if *old_state != RunState::Running
            || parent.record.state != RunState::Running
            || parent.lease.is_none()
            || *expected != current.progress.revision()
        {
            return Err(Error::Conflict);
        }
        if !matches!(
            current
                .execution_definition()?
                .controls
                .get(&claim.node_id)
                .map(|s| &s.operation),
            Some(WorkflowControlOperation::Wait { .. })
        ) {
            return Err(Error::Forbidden);
        }
        let mut next = current.clone();
        next.progress.complete_timer(
            &next.plan,
            *expected,
            claim,
            id,
            next.binding.inputs.as_deref(),
            at_ms,
        )?;
        change.events = vec![
            (if let WorkflowNodeState::WaitingTimer { wake_at, .. } =
                &next.progress.nodes()[&claim.node_id].state
            {
                (
                    "workflow_timer_scheduled".into(),
                    json!({"node_id":claim.node_id,"wake_at":wake_at}),
                )
            } else {
                (
                    "workflow_node_skipped".into(),
                    json!({"node_id":claim.node_id}),
                )
            }),
        ];
        write_progress(change, current, next, true);
    } else if change.workflow_timers_advance {
        if *old_state != RunState::Running
            || parent.record.state != RunState::Running
            || parent.lease.is_none()
        {
            return Err(Error::Forbidden);
        }
        let mut next = current.clone();
        if !next
            .progress
            .advance_timers(&next.plan, next.progress.revision(), at)?
        {
            return Err(Error::Conflict);
        }
        write_progress(change, current, next, true);
    } else if change.workflow_timer_park {
        if *old_state != RunState::Running
            || parent.record.state != RunState::WaitingForTimer
            || parent.lease.is_some()
            || !current.progress.can_park_timer(&current.plan)?
            || current
                .progress
                .next_wake_at()
                .is_none_or(|wake| wake <= at)
        {
            return Err(Error::Conflict);
        }
    } else if change.workflow_timer_wake {
        if *old_state != RunState::WaitingForTimer
            || parent.record.state != RunState::Queued
            || parent.lease.is_some()
            || current.progress.next_wake_at().is_none_or(|wake| wake > at)
        {
            return Err(Error::Conflict);
        }
        if time_budget_exhausted(&parent.record, at_ms) {
            parent.transition(RunState::Expired)?;
            parent.record.error_code = Some("budget_exceeded".into());
            let mut next = current.clone();
            next.progress.close_timers(
                &next.plan,
                next.progress.revision(),
                "budget_exceeded",
                at,
            )?;
            write_progress(change, current, next, false);
            change.events = vec![(
                "workflow_timer_expired".into(),
                json!({"state":"expired","no_new_action_started":true}),
            )];
            return Ok(());
        }
        registry::lock_head(tx, &current.project_id, &current.binding.workflow_id).await?;
        if registry::read_entry(tx, &current.project_id, &current.binding.workflow_id)
            .await?
            .revoked
        {
            parent.transition(RunState::Failed)?;
            parent.record.error_code = Some("workflow_revoked".into());
            let mut next = current.clone();
            next.progress.close_timers(
                &next.plan,
                next.progress.revision(),
                "workflow_revoked",
                at,
            )?;
            write_progress(change, current, next, false);
            change.events = vec![(
                "workflow_timer_revoked".into(),
                json!({"state":"failed","no_new_action_started":true}),
            )];
        }
    } else if let Some(reason) = close_reason {
        if !time_budget_exhausted(&parent.record, at_ms)
            || parent.record.error_code.as_deref() != Some("budget_exceeded")
        {
            return Err(Error::Conflict);
        }
        let mut next = current.clone();
        next.progress
            .close_timers(&next.plan, next.progress.revision(), reason, at)?;
        write_progress(change, current, next, false);
    }
    Ok(())
}
pub(super) async fn write_projection(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &StoredRun,
    _change: &Change,
) -> Result<()> {
    if parent.record.spec.workflow.is_none() {
        return Ok(());
    }
    if parent.record.state == RunState::WaitingForTimer {
        let workflow = runs::load(tx, &parent.record).await?;
        if !workflow.progress.can_park_timer(&workflow.plan)? {
            return Err(Error::Forbidden);
        }
        sqlx::query("INSERT INTO workflow_timer_wakes(run_id,progress_revision,progress_sha256,wake_at) VALUES($1,$2,$3,$4) ON CONFLICT(run_id) DO UPDATE SET progress_revision=$2,progress_sha256=$3,wake_at=$4")
            .bind(&parent.record.id).bind(workflow.progress.revision() as i64).bind(canonical_hash(&workflow.progress)).bind(workflow.progress.next_wake_at().ok_or(Error::Forbidden)?)
            .execute(&mut **tx).await.map_err(storage_error)?;
    } else {
        sqlx::query("DELETE FROM workflow_timer_wakes WHERE run_id=$1")
            .bind(&parent.record.id)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    Ok(())
}
