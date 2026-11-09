//! Native graph coordinator for trusted starts/workers; product API and tool IO remain separate.
use super::*;
use cowork_domain::workflows::{WorkflowClaim, WorkflowState};
use cowork_storage::{WorkflowCommand, WorkflowCommandResult};
use std::collections::BTreeMap;
impl Runtime {
    /// Claims one queued parent exactly once. Duplicate controllers never inherit
    /// its lease or interrupt the existing owner. Terminal reads perform no IO.
    pub async fn run_native_workflow(
        &self,
        project: &str,
        run: &str,
        stop: &CancellationToken,
    ) -> Result<RunRecord> {
        self.store.stored_workflow(project, run).await?;
        let current = self.store.stored_run(run).await?;
        if matches!(current.record.state, RunState::Completed | RunState::Failed) {
            return Ok(self
                .store
                .finish_workflow_run(project, run, "read-receipt")
                .await?
                .record);
        }
        if matches!(current.record.state, RunState::Canceled | RunState::Expired) {
            return Ok(current.record);
        }
        if stop.is_cancelled() {
            return Err(Error::Provider("shutdown".into()));
        }
        let parent = self.store.claim_workflow_run(project, run).await?;
        self.execute_claimed_workflow(parent, stop).await
    }
    /// The common queue already committed the one parent lease and folder owner.
    /// This entry accepts only that exact live ownership; it never claims again.
    pub(super) async fn execute_claimed_workflow(
        &self,
        parent: StoredRun,
        stop: &CancellationToken,
    ) -> Result<RunRecord> {
        let project = &parent.record.spec.project_id;
        let run = &parent.record.id;
        if parent.record.spec.workflow.is_none() {
            return Err(Error::Forbidden);
        }
        let lease = parent
            .lease
            .as_ref()
            .ok_or(Error::StaleLease)?
            .token
            .clone();
        let owned = stop.child_token();
        let result = self.coordinate_workflow(project, run, &lease, &owned).await;
        owned.cancel();
        match result {
            Ok(record) => Ok(record),
            Err(error) => {
                if let Some(record) =
                    Box::pin(self.close_workflow_error(project, run, &lease, error.code())).await?
                {
                    Ok(record)
                } else {
                    Err(error)
                }
            }
        }
    }
    // Cancellation is an explicit known intent. The store's workflow terminal
    // fence distinguishes human waits from unresolved model/tool effects.
    async fn acknowledge_workflow_cancel(&self, run: &str, lease: &str) -> Result<RunRecord> {
        Ok(self
            .store
            .acknowledge_workflow_cancellation(run, lease)
            .await?
            .record)
    }

    async fn close_workflow_error(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        code: &str,
    ) -> Result<Option<RunRecord>> {
        for _ in 0..16 {
            let current = self.store.stored_run(run).await?;
            if current.record.spec.project_id != project {
                return Err(Error::NotFound);
            }
            if current.record.state == RunState::Canceled {
                return Ok(Some(current.record));
            }
            if !matches!(
                current.record.state,
                RunState::Running | RunState::CancelRequested | RunState::WaitingForSnapshot
            ) || current
                .lease
                .as_ref()
                .is_none_or(|owned| owned.token != lease)
            {
                return Ok(None);
            }
            if current.record.state == RunState::CancelRequested {
                match Box::pin(self.acknowledge_workflow_cancel(run, lease)).await {
                    Ok(record) => return Ok(Some(record)),
                    Err(Error::Conflict | Error::StaleLease) => continue,
                    Err(error) => return Err(error),
                }
            }
            match self.store.interrupt_workflow_run(run, lease, code).await {
                Ok(_) => return Ok(None),
                Err(Error::Conflict | Error::StaleLease) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(Error::Conflict)
    }
    async fn coordinate_workflow(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        stop: &CancellationToken,
    ) -> Result<RunRecord> {
        let mut tasks = JoinSet::new();
        // Human continuations occupy a slot without an invented model reservation.
        let mut active: BTreeMap<String, (WorkflowClaim, Option<String>)> = BTreeMap::new();
        let initial = self.store.stored_run(run).await?;
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(remaining_run_millis(&initial.record, now()));
        let terminal = matches!(
            self.store
                .stored_workflow(project, run)
                .await?
                .progress
                .state(),
            WorkflowState::Completed | WorkflowState::Failed
        );
        if terminal {
            if initial.record.id != run {
                return Err(Error::Forbidden);
            }
            if initial.record.spec.project_id != project {
                return Err(Error::NotFound);
            }
            initial.assert_lease(lease, now())?;
            if initial.record.state == RunState::CancelRequested {
                return Box::pin(self.acknowledge_workflow_cancel(run, lease)).await;
            }
            if initial.record.state != RunState::Running {
                return Err(Error::UncertainEffect);
            }
            if stop.is_cancelled() {
                return Err(Error::Provider("shutdown".into()));
            }
            // The store revalidates the terminal graph, settled effects, clock
            // and exact lease in its result transaction. No new agent execution
            // is needed for already confirmed results.
            let finished = self.store.finish_workflow_run(project, run, lease).await?;
            if finished.record.state == RunState::WaitingForSnapshot {
                self.restore_run_workspace(&finished.record).await?;
                self.finish_workspace(run, lease, stop, deadline).await?;
            }
            return Ok(self.store.stored_run(run).await?.record);
        }
        self.store
            .assert_workflow_clock_authority(project, run, lease)
            .await?;
        self.restore_run_workspace(&initial.record).await?;
        let activated = self.store.stored_workflow(project, run).await?;
        if let Some(c) = activated
            .progress
            .human_continuations()
            .last()
            .filter(|c| c.lease_sha256.as_deref() == Some(hash(lease.as_bytes()).as_str()))
        {
            for cursor in &c.cursors {
                let claim = c
                    .execution_claims
                    .iter()
                    .find(|q| q.node_id == cursor.node_id)
                    .ok_or(Error::Forbidden)?
                    .clone();
                if c.consumed.contains_key(&claim.node_id) {
                    return Err(Error::UncertainEffect);
                }
                let turn = self
                    .store
                    .workflow_model_turn(project, run, &cursor.model_turn_id)
                    .await?
                    .ok_or(Error::Forbidden)?;
                active.insert(claim.node_id.clone(), (claim.clone(), None));
                let runtime = self.clone();
                let project = project.to_owned();
                let run = run.to_owned();
                let lease = lease.to_owned();
                let stop = stop.clone();
                tasks.spawn(async move {
                    let result = Box::pin(runtime.execute_workflow_node_with_turn(
                        &project,
                        &run,
                        &lease,
                        &claim,
                        &id(),
                        Some(turn),
                        &stop,
                    ))
                    .await;
                    (claim.node_id, result)
                });
            }
        }
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        let mut last_heartbeat = tokio::time::Instant::now();
        loop {
            let workflow = self.store.stored_workflow(project, run).await?;
            let parent = self.store.stored_run(run).await?;
            if parent.record.state == RunState::Canceled {
                return Ok(parent.record);
            }
            parent.assert_lease(lease, now())?;
            if parent.record.state == RunState::CancelRequested {
                return Box::pin(self.acknowledge_workflow_cancel(run, lease)).await;
            }
            if parent.record.state != RunState::Running {
                return Err(Error::UncertainEffect);
            }
            if stop.is_cancelled() {
                return Err(Error::Provider("shutdown".into()));
            }
            if active.is_empty()
                && matches!(
                    workflow.progress.state(),
                    WorkflowState::Completed | WorkflowState::Failed
                )
            {
                let finished = self.store.finish_workflow_run(project, run, lease).await?;
                if finished.record.state == RunState::WaitingForSnapshot {
                    self.finish_workspace(run, lease, stop, deadline).await?;
                }
                return Ok(self.store.stored_run(run).await?.record);
            }
            if workflow.progress.state() == WorkflowState::Interrupted {
                return Err(Error::UncertainEffect);
            }
            self.assert_snapshot_controller(&parent.record.spec).await?;
            if time_budget_exhausted(&parent.record, now()) {
                return Err(Error::BudgetExceeded);
            }
            if workflow.progress.next_wake_at().is_some()
                && self
                    .store
                    .advance_workflow_timers(project, run, lease)
                    .await?
            {
                continue;
            }
            let ready = workflow.progress.ready_nodes(&workflow.plan)?;
            // Until a spawned sender commits its model reservation, account for
            // that same context window locally. Never overfill the shared pool.
            let unadmitted = active
                .values()
                .filter(|(_, attempt)| {
                    attempt
                        .as_ref()
                        .is_some_and(|id| !workflow.progress.attempts().contains_key(id))
                })
                .map(|(claim, _)| {
                    let node = &workflow.progress.nodes()[&claim.node_id];
                    node.plan
                        .role_id
                        .as_ref()
                        .map(|role| u64::from(workflow.plan.roles[role].provider.context_tokens))
                        .unwrap_or(0)
                })
                .fold(0u64, u64::saturating_add);
            let mut available = workflow.plan.budgets.max_total_tokens.saturating_sub(
                workflow
                    .progress
                    .charged_tokens()
                    .saturating_add(unadmitted),
            );
            for node in &ready {
                if active.len() >= usize::from(workflow.plan.max_parallel) {
                    break;
                }
                let context = if workflow.progress.requires_native(&workflow.plan, node)?
                    || workflow.progress.nodes()[node].plan.role_id.is_none()
                {
                    0
                } else {
                    let role = workflow.progress.nodes()[node].plan.role()?;
                    u64::from(workflow.plan.roles[role].provider.context_tokens)
                };
                if context > available && !active.is_empty() {
                    continue;
                }
                // If the pool is definitively exhausted with no active sender,
                // the normal admission records a known budget failure, no IO.
                let Some(claim) = self.claim_workflow_node(project, run, lease, node).await? else {
                    continue;
                };
                let attempt = id();
                available = available.saturating_sub(context);
                active.insert(node.clone(), (claim.clone(), Some(attempt.clone())));
                let runtime = self.clone();
                let project = project.to_owned();
                let run = run.to_owned();
                let lease = lease.to_owned();
                let stop = stop.clone();
                tasks.spawn(async move {
                    let result = runtime
                        .execute_workflow_node(&project, &run, &lease, &claim, &attempt, &stop)
                        .await;
                    (claim.node_id, result)
                });
            }
            if active.is_empty() {
                if workflow.progress.can_park_timer(&workflow.plan)? {
                    match self.store.park_workflow_timer(project, run, lease).await {
                        Ok(parent) => return Ok(parent.record),
                        Err(Error::Conflict) => continue,
                        Err(error) => return Err(error),
                    }
                }
                return Err(Error::Conflict);
            }
            tokio::select! {
                biased;
                _ = stop.cancelled() => return Err(Error::Provider("shutdown".into())),
                _ = tokio::time::sleep_until(deadline) => return Err(Error::BudgetExceeded),
                Some(joined) = tasks.join_next() => {
                    let (node, result) = joined.map_err(storage_error)?;
                    active.remove(&node);
                    result?;
                },
                _ = interval.tick() => {
                    if last_heartbeat.elapsed() >= Duration::from_secs(3) {
                        self.store.heartbeat(run, lease).await?;
                        last_heartbeat = tokio::time::Instant::now();
                    }
                },
            }
        }
    }
    async fn claim_workflow_node(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        node: &str,
    ) -> Result<Option<WorkflowClaim>> {
        for _ in 0..64 {
            let before = self.store.stored_workflow(project, run).await?;
            if !before
                .progress
                .ready_nodes(&before.plan)?
                .iter()
                .any(|id| id == node)
            {
                return Ok(None);
            }
            match self
                .store
                .workflow_command(
                    project,
                    run,
                    lease,
                    before.progress.revision(),
                    WorkflowCommand::Claim {
                        node_id: node.into(),
                    },
                )
                .await
            {
                Ok(WorkflowCommandResult::Claimed(claim)) => return Ok(Some(claim)),
                Ok(_) => return Err(Error::Forbidden),
                Err(Error::Conflict) => tokio::task::yield_now().await,
                Err(error) => return Err(error),
            }
        }
        Err(Error::Conflict)
    }
    async fn execute_workflow_node(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        claim: &WorkflowClaim,
        attempt: &str,
        stop: &CancellationToken,
    ) -> Result<()> {
        let workflow = self.store.stored_workflow(project, run).await?;
        if workflow
            .progress
            .requires_native(&workflow.plan, &claim.node_id)?
        {
            for _ in 0..64 {
                if stop.is_cancelled() {
                    return Err(Error::Provider("shutdown".into()));
                }
                let current = self.store.stored_workflow(project, run).await?;
                match self
                    .store
                    .workflow_command(
                        project,
                        run,
                        lease,
                        current.progress.revision(),
                        WorkflowCommand::Control {
                            claim: claim.clone(),
                            id: attempt.into(),
                        },
                    )
                    .await
                {
                    Ok(_) => return Ok(()),
                    Err(Error::Conflict) => tokio::task::yield_now().await,
                    Err(error) => {
                        return self
                            .finish_workflow_node(project, run, lease, claim, None, error.code())
                            .await;
                    }
                }
            }
            return Err(Error::Conflict);
        }
        let saved = if let Some(cursor) = workflow
            .progress
            .settled_cursors()
            .iter()
            .rev()
            .find(|c| c.execution_claim.as_ref() == Some(claim))
        {
            Some(
                self.store
                    .workflow_model_turn(project, run, &cursor.model_turn_id)
                    .await?
                    .ok_or(Error::Forbidden)?,
            )
        } else {
            None
        };
        Box::pin(
            self.execute_workflow_node_with_turn(project, run, lease, claim, attempt, saved, stop),
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn execute_workflow_node_with_turn(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        claim: &WorkflowClaim,
        attempt: &str,
        mut saved: Option<cowork_storage::WorkflowModelTurn>,
        stop: &CancellationToken,
    ) -> Result<()> {
        let mut attempt = attempt.to_owned();
        loop {
            let turn = if let Some(turn) = saved.take() {
                turn
            } else {
                match self
                    .execute_workflow_model_turn(project, run, lease, claim, &attempt, stop)
                    .await
                {
                    Ok(turn) => turn,
                    Err(error) => {
                        if self
                            .store
                            .workflow_model_turn(project, run, &attempt)
                            .await?
                            .is_some()
                        {
                            return Err(error);
                        }
                        // Definitively before this model admission: no remote request.
                        return self
                            .finish_workflow_node(project, run, lease, claim, None, error.code())
                            .await;
                    }
                }
            };
            let response = turn.response.as_ref().ok_or(Error::UncertainEffect)?;
            if let Some(code) = &response.rejection_code {
                return self
                    .finish_workflow_node(project, run, lease, claim, None, code)
                    .await;
            }
            if response.tools.is_empty()
                && cowork_domain::hybrid_planning(&turn.context.spec, &turn.context.messages)
            {
                attempt = id();
                continue;
            }
            if response.tools.is_empty() {
                return self
                    .finish_workflow_node(
                        project,
                        run,
                        lease,
                        claim,
                        Some(response.content.clone()),
                        "",
                    )
                    .await;
            }
            for call in response.tools.iter().skip(turn.tool_results.len()) {
                let mut conflicts = 0;
                loop {
                    if stop.is_cancelled() {
                        return Err(Error::Provider("shutdown".into()));
                    }
                    let parent = self.store.stored_run(run).await?;
                    parent.assert_lease(lease, now())?;
                    ensure_running(&parent)?;
                    let result = if workflow_documents::document_tool(&call.name) {
                        Box::pin(self.invoke_workflow_document(
                            project, run, lease, &turn.id, &call.id, stop,
                        ))
                        .await
                    } else if workflow_external::external(&call.name)
                        || (call.name == "execute_program"
                            && parent.record.spec.environment.is_some())
                    {
                        self.invoke_workflow_external(project, run, lease, &turn.id, &call.id, stop)
                            .await
                    } else if toolbox_bridge::is_native_tool(&call.name) {
                        self.invoke_workflow_toolbox(project, run, lease, &turn.id, &call.id, stop)
                            .await
                    } else {
                        self.store
                            .execute_workflow_artifact_tool(project, run, lease, &turn.id, &call.id)
                            .await
                    };
                    match result {
                        Ok(cowork_storage::WorkflowToolExecution::Completed(receipt)) => {
                            if call.name == "delegate_agents" && receipt.succeeded {
                                return Ok(());
                            }
                            break;
                        }
                        Ok(cowork_storage::WorkflowToolExecution::AwaitingApproval(_))
                        | Ok(cowork_storage::WorkflowToolExecution::AwaitingInput(_)) => {
                            tokio::select! {
                                _ = stop.cancelled() => return Err(Error::Provider("shutdown".into())),
                                _ = tokio::time::sleep(Duration::from_millis(100)) => {},
                            }
                        }
                        Err(Error::Conflict) if conflicts < 64 => {
                            conflicts += 1;
                            tokio::task::yield_now().await;
                        }
                        Err(Error::Provider(code))
                            if code == "workflow_tools_unavailable"
                                || code == "workflow_stopped" =>
                        {
                            return self
                                .finish_workflow_node(project, run, lease, claim, None, &code)
                                .await;
                        }
                        Err(Error::Validation(_)) => {
                            return self
                                .finish_workflow_node(
                                    project,
                                    run,
                                    lease,
                                    claim,
                                    None,
                                    "invalid_request",
                                )
                                .await;
                        }
                        Err(Error::BudgetExceeded) => {
                            // No database-only tool effect was committed. This is
                            // a known node failure; independent senders may settle.
                            return self
                                .finish_workflow_node(
                                    project,
                                    run,
                                    lease,
                                    claim,
                                    None,
                                    "budget_exceeded",
                                )
                                .await;
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            // Every proposed call is confirmed before constructing a new context.
            if turn.deterministic {
                let saved = self
                    .store
                    .workflow_model_turn(project, run, &turn.id)
                    .await?
                    .ok_or(Error::Forbidden)?;
                return self
                    .finish_workflow_node(
                        project,
                        run,
                        lease,
                        claim,
                        cowork_storage::deterministic_step_output(&saved),
                        if saved.context.spec.provider.is_none() {
                            "native_document_result_invalid"
                        } else {
                            "pipeline_web_failed"
                        },
                    )
                    .await;
            }
            attempt = id();
        }
    }
    async fn finish_workflow_node(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        claim: &WorkflowClaim,
        output: Option<String>,
        code: &str,
    ) -> Result<()> {
        for _ in 0..64 {
            let before = self.store.stored_workflow(project, run).await?;
            let command = if let Some(output) = &output {
                WorkflowCommand::Complete {
                    claim: claim.clone(),
                    output: output.clone(),
                }
            } else {
                WorkflowCommand::Fail {
                    claim: claim.clone(),
                    code: code.into(),
                }
            };
            match self
                .store
                .workflow_command(project, run, lease, before.progress.revision(), command)
                .await
            {
                Ok(WorkflowCommandResult::Confirmed) => return Ok(()),
                Ok(_) => return Err(Error::Forbidden),
                Err(Error::Conflict) => tokio::task::yield_now().await,
                Err(error) => return Err(error),
            }
        }
        Err(Error::Conflict)
    }
}
