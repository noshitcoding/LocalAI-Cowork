use super::*;
impl Store {
    /// Read-only dispatch fence; it never claims work or advances a clock independently.
    pub async fn assert_workflow_clock_authority(
        &self,
        project: &str,
        run: &str,
        lease: &str,
    ) -> Result<()> {
        let mut tx = self.snapshot_transaction().await?;
        let payload: Option<String> = sqlx::query_scalar("SELECT payload FROM runs WHERE id=$1")
            .bind(run)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let parent: StoredRun = parse(&payload.ok_or(Error::NotFound)?)?;
        if parent.record.id != run {
            return Err(Error::Forbidden);
        }
        if parent.record.spec.project_id != project {
            return Err(Error::NotFound);
        }
        if parent.record.spec.workflow.is_none() {
            return Err(Error::Forbidden);
        }
        let workflow = load(&mut tx, &parent.record).await?;
        Box::pin(super::continuations::verify_history(
            &mut tx, &parent, &workflow,
        ))
        .await?;
        if registry::read_entry(&mut tx, project, &workflow.binding.workflow_id)
            .await?
            .revoked
        {
            return Err(Error::WorkflowRevoked);
        }
        super::tools::reject_stopped(&workflow)?;
        let sampled = self
            .workflow_clock
            .execution_now(&mut tx, &parent.record)
            .await?;
        if parent.record.state != RunState::Running {
            return Err(Error::Conflict);
        }
        parent.assert_lease(lease, sampled)?;
        if time_budget_exhausted(&parent.record, sampled) {
            return Err(Error::BudgetExceeded);
        }
        tx.commit().await.map_err(storage_error)?;
        let fresh = self.workflow_clock.now();
        if fresh < sampled {
            return Err(Error::WorkflowClockRollback);
        }
        parent.assert_lease(lease, fresh)?;
        if time_budget_exhausted(&parent.record, fresh) {
            return Err(Error::BudgetExceeded);
        }
        Ok(())
    }

    /// Conservative cleanup is bound to the currently owned token, including an expired one.
    /// It cannot grant new rights, renew ownership or confirm unknown external effects.
    pub async fn interrupt_workflow_run(
        &self,
        run: &str,
        lease: &str,
        code: &str,
    ) -> Result<StoredRun> {
        valid_text(code, 128)?;
        self.mutate_with_clock_intent(
            run,
            Some(lease),
            crate::workflow_clocks::WorkflowClockIntent::Recovery,
            |parent| {
                if parent.record.spec.workflow.is_none()
                    || !matches!(
                        parent.record.state,
                        RunState::Running | RunState::WaitingForSnapshot
                    )
                {
                    return Err(Error::Conflict);
                }
                parent.transition(RunState::Interrupted)?;
                parent.record.error_code = Some(code.into());
                parent.lease = None;
                Ok(Change::event(
                    "workflow_interrupted",
                    json!({"reason":code,"automatic_retry":false}),
                ))
            },
        )
        .await
    }

    /// Only a real cancel-request prefix authorizes worker cancellation cleanup.
    pub async fn acknowledge_workflow_cancellation(
        &self,
        run: &str,
        lease: &str,
    ) -> Result<StoredRun> {
        let before = self.stored_run(run).await?;
        if before.record.spec.workflow.is_none() {
            return Err(Error::Forbidden);
        }
        if before.record.state == RunState::Canceled {
            return Ok(before);
        }
        let canceled = self
            .mutate_with_clock_intent(
                run,
                Some(lease),
                crate::workflow_clocks::WorkflowClockIntent::Cancellation,
                |parent| {
                    if parent.record.spec.workflow.is_none()
                        || parent.record.state != RunState::CancelRequested
                    {
                        return Err(Error::Conflict);
                    }
                    parent.transition(RunState::Canceled)?;
                    parent.lease = None;
                    Ok(Change::event("run_canceled", json!({"state":"canceled"})))
                },
            )
            .await;
        match canceled {
            Err(Error::UncertainEffect) => self.mutate_with_clock_intent(
                run, Some(lease), crate::workflow_clocks::WorkflowClockIntent::Cancellation, |parent| {
                    if parent.record.spec.workflow.is_none() || parent.record.state != RunState::CancelRequested {
                        return Err(Error::Conflict);
                    }
                    parent.transition(RunState::Interrupted)?;
                    parent.record.error_code = Some("canceled_with_uncertain_effect".into());
                    parent.lease = None;
                    Ok(Change::event("workflow_cancel_interrupted",
                        json!({"reason":"canceled_with_uncertain_effect","automatic_retry":false})))
                }
            ).await,
            other => other,
        }
    }
    /// Trusted start, atomically sharing the ordinary order/message/event path.
    /// Trusted creation; no current public workflow API/UI starts these workflows.
    pub async fn create_workflow_run(
        &self,
        input: &CreateWorkflowRun,
        spec: RunSpec,
    ) -> Result<RunRecord> {
        if spec.provider.is_some()
            || spec.file_action.is_some()
            || spec.workflow.is_some()
            || spec.order_id != input.order_id
        {
            return Err(Error::Forbidden);
        }
        let request_hash = canonical_hash(&json!({"workflow":input,"spec":spec}));
        let request = RunRequest {
            order_id: input.order_id.clone(),
            base_revision: input.base_revision,
            idempotency_key: input.idempotency_key.clone(),
            prompt: spec.prompt.clone(),
            workflow: Some(WorkflowStart {
                workflow_id: input.workflow_id.clone(),
                version_id: input.version_id.clone(),
                max_parallel: input.max_parallel,
                product: false,
                expected_binding: None,
            }),
        };
        self.insert_run(
            &request,
            spec,
            request_hash,
            false,
            json!({"kind":"trusted_workflow","request":input}),
        )
        .await
    }
    pub async fn stored_workflow(&self, project: &str, run_id: &str) -> Result<StoredWorkflow> {
        let mut tx = self.snapshot_transaction().await?;
        let payload: Option<String> = sqlx::query_scalar("SELECT payload FROM runs WHERE id=$1")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let run: StoredRun = parse(&payload.ok_or(Error::NotFound)?)?;
        if run.record.spec.project_id != project {
            return Err(Error::NotFound);
        }
        let result = load(&mut tx, &run.record).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn claim_workflow_run(&self, project: &str, run_id: &str) -> Result<StoredRun> {
        self.stored_workflow(project, run_id).await?;
        let result = self.claim_queued_run(run_id, Some(project), true).await;
        if matches!(
            &result,
            Err(Error::WorkflowRevoked
                | Error::WorkflowClockRollback
                | Error::WorkflowClockUnverified
                | Error::BudgetExceeded
                | Error::Forbidden)
        ) {
            self.close_unactivated_human_resume(run_id).await?;
        }
        result
    }
    /// Only the newly committed action returns its one-shot permit. Pure planning
    /// is never an authorization; caller still uses the exact shared IO broker.
    pub async fn workflow_command(
        &self,
        project: &str,
        run_id: &str,
        lease: &str,
        expected: u64,
        command: WorkflowCommand,
    ) -> Result<WorkflowCommandResult> {
        let before = self.stored_workflow(project, run_id).await?;
        agents::check_command(&before, &command)?;
        let mut next = before.clone();
        let timestamp = seconds();
        let result = match &command {
            WorkflowCommand::Claim { node_id } => WorkflowCommandResult::Claimed(
                next.progress
                    .claim(&next.plan, expected, node_id, timestamp)?,
            ),
            WorkflowCommand::Admit {
                claim,
                id,
                operation,
            } => WorkflowCommandResult::Admission(next.progress.admit(
                &next.plan,
                expected,
                claim,
                id,
                operation.clone(),
                timestamp,
            )?),
            WorkflowCommand::Settle { claim, id, outcome } => {
                next.progress.settle(
                    &next.plan,
                    expected,
                    claim,
                    id,
                    outcome.clone(),
                    timestamp,
                )?;
                WorkflowCommandResult::Confirmed
            }
            WorkflowCommand::Complete { claim, output } => {
                next.progress
                    .complete(&next.plan, expected, claim, output, timestamp)?;
                WorkflowCommandResult::Confirmed
            }
            WorkflowCommand::Fail { claim, code } => {
                next.progress
                    .fail(&next.plan, expected, claim, code, timestamp)?;
                WorkflowCommandResult::Confirmed
            }
            WorkflowCommand::Delegate {
                claim,
                id,
                requests,
            } => WorkflowCommandResult::Delegated(next.progress.delegate(
                &next.plan,
                expected,
                claim,
                id,
                requests.clone(),
                timestamp,
            )?),
        };
        if next.progress.revision() == before.progress.revision() {
            // An existing admission/settlement is a read receipt, never IO authority.
            return Ok(result);
        }
        let new_action = matches!(
            command,
            WorkflowCommand::Claim { .. }
                | WorkflowCommand::Admit { .. }
                | WorkflowCommand::Delegate { .. }
        );
        let write = WorkflowWrite {
            new_action,
            expected_revision: before.progress.revision(),
            expected_sha256: canonical_hash(&before.progress),
            next,
        };
        self.mutate(run_id,Some(lease),|r| {
            if r.record.spec.project_id!=project || !matches!(r.record.state,RunState::Running|RunState::CancelRequested) {return Err(Error::Conflict);}
            if new_action && (r.record.state!=RunState::Running || time_budget_exhausted(&r.record,now())) {return Err(Error::BudgetExceeded);}
            let mut change=Change::event("workflow_updated",json!({"workflow_id":write.next.binding.workflow_id,"revision":write.next.progress.revision(),"state":write.next.progress.state(),"charged_tokens":write.next.progress.charged_tokens()}));
            for (request, checkpoint) in write.next.progress.input_checkpoints() {
                if let Some(reason) = &checkpoint.closed_reason
                    && before.progress.input_checkpoints().get(request).is_some_and(|old|old.closed_reason.is_none()) {
                    change.events.push(("workflow_input_closed".into(),json!({"request_id":request,"node_id":checkpoint.claim.node_id,"reason":reason})));
                }
            }
            change.workflow_write=Some(WorkflowWrite {new_action:write.new_action,expected_revision:write.expected_revision,expected_sha256:write.expected_sha256.clone(),next:write.next.clone()});
            if write.next.progress.state()==WorkflowState::Interrupted {r.transition(RunState::Interrupted)?;r.lease=None;r.record.error_code=Some("workflow_interrupted".into());}
            Ok(change)
        }).await?;
        Ok(result)
    }
}
pub(crate) async fn initialize(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    spec: &mut RunSpec,
    start: &WorkflowStart,
) -> Result<WorkflowPlan> {
    registry::lock_head(tx, &spec.project_id, &start.workflow_id).await?;
    let entry = registry::read_entry(tx, &spec.project_id, &start.workflow_id).await?;
    if entry.revoked {
        return Err(Error::Forbidden);
    }
    let version =
        registry::read_version(tx, &spec.project_id, &start.workflow_id, &start.version_id).await?;
    if start.product {
        product::validate_definition(&version.definition)?;
        if start.max_parallel != 4
            || spec.provider.is_some()
            || spec.file_action.is_some()
            || (spec.environment.is_none()
                && spec.executor
                    != if spec
                        .allowed_tools
                        .iter()
                        .any(|name| product::NATIVE_TOOLS.contains(&name.as_str()))
                    {
                        "owner-native-toolbox-v1"
                    } else {
                        "local-v2"
                    })
            || spec.policy_revision != 1
            || !product::valid_product_tools(&spec.allowed_tools)
        {
            return Err(Error::Forbidden);
        }
    }
    let mut profiles = vec![];
    let ids: std::collections::BTreeSet<_> = version
        .definition
        .roles
        .iter()
        .map(|r| r.provider_id.clone())
        .collect();
    for provider_id in ids {
        if sqlx::query("UPDATE providers SET payload=payload WHERE id=$1")
            .bind(&provider_id)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
            != 1
        {
            return Err(Error::Provider("profile_missing".into()));
        }
        let value: String = sqlx::query_scalar("SELECT payload FROM providers WHERE id=$1")
            .bind(&provider_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
        let profile: ProviderProfile = parse(&value)?;
        if profile.id != provider_id {
            return Err(Error::Forbidden);
        }
        profiles.push(profile);
    }
    let rights = WorkflowRights {
        tools: spec.allowed_tools.iter().cloned().collect(),
        folders: spec.folders.iter().map(|f| f.id.clone()).collect(),
    };
    if rights.tools.len() != spec.allowed_tools.len()
        || rights.folders.len() != spec.folders.len()
        || spec.folders.iter().any(|f| {
            f.project_id != spec.project_id
                || f.order_id.as_ref().is_some_and(|o| o != &spec.order_id)
        })
    {
        return Err(Error::Forbidden);
    }
    let scope = WorkflowScope {
        rights,
        budgets: spec.budgets.clone(),
        max_parallel: start.max_parallel,
    };
    let plan = WorkflowPlan::compile(&version.definition, &scope, &profiles)?;
    for role in plan.roles.values() {
        let mut bound = spec.clone();
        bound.provider = Some(role.provider.clone());
        bound.model_binding = role.model_binding.clone();
        crate::model_registry::check_binding(tx, &bound).await?;
    }
    spec.budgets = plan.budgets.clone();
    spec.workflow = Some(WorkflowBinding {
        workflow_id: start.workflow_id.clone(),
        version_id: start.version_id.clone(),
        definition_sha256: plan.source_sha256.clone(),
        plan_sha256: plan.sha256.clone(),
    });
    if start.expected_binding.as_ref().is_some_and(|expected| {
        spec.workflow
            .as_ref()
            .is_none_or(|binding| canonical_hash(binding) != canonical_hash(expected))
    }) {
        return Err(Error::Conflict);
    }
    Ok(plan)
}
pub(crate) async fn insert(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &RunRecord,
    plan: WorkflowPlan,
    clock: &crate::workflow_clocks::WorkflowClock,
) -> Result<()> {
    let workflow = StoredWorkflow {
        format_version: 1,
        run_id: run.id.clone(),
        project_id: run.spec.project_id.clone(),
        binding: run.spec.workflow.clone().ok_or(Error::Forbidden)?,
        progress: WorkflowProgress::new(&plan)?,
        agents: Default::default(),
        inputs: Default::default(),
        samplings: Default::default(),
        plan,
    };
    workflow.validate(run)?;
    sqlx::query("INSERT INTO workflow_runs(run_id,workflow_id,version_id,project_id,revision,plan_sha256,progress_sha256,payload) VALUES($1,$2,$3,$4,0,$5,$6,$7)").bind(&run.id).bind(&workflow.binding.workflow_id).bind(&workflow.binding.version_id).bind(&workflow.project_id).bind(&workflow.plan.sha256).bind(canonical_hash(&workflow.progress)).bind(json_string(&workflow)?).execute(&mut **tx).await.map_err(storage_error)?;
    clock.seed(tx, run, &workflow).await?;
    Ok(())
}
pub(crate) async fn load(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &RunRecord,
) -> Result<StoredWorkflow> {
    let row=sqlx::query("SELECT revision,plan_sha256,progress_sha256,payload FROM workflow_runs WHERE run_id=$1 AND project_id=$2").bind(&run.id).bind(&run.spec.project_id).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let workflow: StoredWorkflow =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    workflow.validate(run)?;
    if workflow.progress.revision() as i64
        != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || workflow.plan.sha256
            != row
                .try_get::<String, _>("plan_sha256")
                .map_err(storage_error)?
        || canonical_hash(&workflow.progress)
            != row
                .try_get::<String, _>("progress_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(workflow)
}
fn pending(workflow: &StoredWorkflow) -> bool {
    workflow.progress.attempts().values().any(|a| {
        (a.outcome.is_none() && !workflow.progress.input_checkpoints().contains_key(&a.id))
            || a.outcome == Some(WorkflowOutcome::Uncertain)
    })
}
/// Runs inside the same transaction as the parent record and sequenced event.
pub(crate) async fn prepare(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    stored: &mut StoredRun,
    change: &mut Change,
    old_state: &RunState,
    conservative_recovery_at: Option<i64>,
) -> Result<()> {
    if stored.record.spec.workflow.is_none() {
        if matches!(old_state, RunState::Queued | RunState::WaitingForExecutor)
            && stored.record.state == RunState::Running
            && sqlx::query_scalar::<_, String>("SELECT run_id FROM workflow_runs WHERE run_id=$1")
                .bind(&stored.record.id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(storage_error)?
                .is_some()
        {
            return Err(Error::Forbidden);
        }
        if change.workflow_write.is_some() || change.workflow_finish {
            return Err(Error::Forbidden);
        }
        return Ok(());
    }
    let current = load(tx, &stored.record).await?;
    let mut historic_parent = stored.clone();
    historic_parent.record.state = old_state.clone();
    if matches!(old_state, RunState::Queued | RunState::WaitingForExecutor) {
        historic_parent.lease = None;
    }
    Box::pin(super::continuations::verify_history(
        tx,
        &historic_parent,
        &current,
    ))
    .await?;
    if change.workflow_resume_block.is_some() {
        if !matches!(
            old_state,
            RunState::Queued
                | RunState::WaitingForExecutor
                | RunState::Canceled
                | RunState::Expired
        ) || stored.lease.is_some()
        {
            return Err(Error::Forbidden);
        }
        registry::lock_head(tx, &current.project_id, &current.binding.workflow_id).await?;
        let clock = crate::workflow_clocks::read_highwater_for_resume(tx, &stored.record).await?;
        let reason = super::continuations::block_reason(tx, stored, &current, now(), clock)
            .await?
            .ok_or(Error::Conflict)?;
        let c = current
            .progress
            .human_continuations()
            .last()
            .ok_or(Error::Forbidden)?;
        crate::workflow_operations::verify_resume_accepted(tx, &stored.record, &current, c).await?;
        let mut next = current.clone();
        next.progress.block_human(
            &next.plan,
            current.progress.revision(),
            &c.id,
            c.generation,
            &reason,
            conservative_recovery_at.ok_or(Error::Forbidden)?,
        )?;
        if !terminal(&stored.record.state) {
            stored.record.state = RunState::Interrupted;
            stored.record.error_code = Some(reason.clone());
        }
        change.workflow_resume_block = Some(reason.clone());
        change.events = vec![(
            "workflow_input_resume_blocked".into(),
            json!({"resume_id":c.id,"generation":c.generation,"reason":reason}),
        )];
        change.workflow_write = Some(WorkflowWrite {
            new_action: false,
            expected_revision: current.progress.revision(),
            expected_sha256: canonical_hash(&current.progress),
            next,
        });
    }
    if matches!(old_state, RunState::Queued | RunState::WaitingForExecutor)
        && stored.record.state == RunState::Running
        && let Some(c) = current
            .progress
            .human_continuations()
            .last()
            .filter(|c| c.execution_claims.is_empty() && c.blocked_reason.is_none())
    {
        let mut queued = stored.clone();
        queued.record.state = old_state.clone();
        queued.lease = None;
        let cursors = super::continuations::verify_live(tx, &queued, &current, now()).await?;
        if cursors != c.cursors {
            return Err(Error::Conflict);
        }
        crate::workflow_operations::verify_resume_accepted(tx, &queued.record, &current, c).await?;
        let lease = stored.lease.as_ref().ok_or(Error::StaleLease)?;
        let mut next = current.clone();
        next.progress.activate_human(
            &next.plan,
            current.progress.revision(),
            &c.id,
            c.generation,
            &lease.token,
            seconds(),
        )?;
        change.workflow_write = Some(WorkflowWrite {
            new_action: true,
            expected_revision: current.progress.revision(),
            expected_sha256: canonical_hash(&current.progress),
            next,
        });
        change.workflow_tool_deadline = c.cursors.iter().map(|c| c.expires_at).min();
        change.events.retain(|(kind, _)| kind != "workflow_started");
        change.events.push(("workflow_input_resume_activated".into(),json!({"resume_id":c.id,"generation":c.generation,"lease_sha256":hash(lease.token.as_bytes()),"claims_sha256":canonical_hash(&change.workflow_write.as_ref().ok_or(Error::Forbidden)?.next.progress.human_continuations().last().ok_or(Error::Forbidden)?.execution_claims)})));
    }
    if let Some(turn) = &change.workflow_model_dispatch_gate {
        if stored.record.state != RunState::Running
            || change.workflow_write.is_some()
            || change.workflow_tool_gate.is_some()
            || change.workflow_finish
        {
            return Err(Error::Forbidden);
        }
        registry::lock_head(
            tx,
            &stored.record.spec.project_id,
            &current.binding.workflow_id,
        )
        .await?;
        if registry::read_entry(
            tx,
            &stored.record.spec.project_id,
            &current.binding.workflow_id,
        )
        .await?
        .revoked
        {
            return Err(Error::WorkflowRevoked);
        }
        super::tools::reject_stopped(&current)?;
        let saved = agents::find_turn(&current, &turn.id).ok_or(Error::NotFound)?;
        if saved.response.is_some() || canonical_hash(saved) != canonical_hash(turn) {
            return Err(Error::Conflict);
        }
        agents::active(&current, &turn.claim)?;
        change.workflow_tool_deadline = Some(super::tools::tool_deadline(stored, &current, turn)?);
    }
    if let Some(gate) = &change.workflow_tool_gate {
        change.workflow_tool_deadline = Some(tools::verify_gate(tx, stored, &current, gate).await?);
    }
    let finish_only = matches!(
        current.progress.state(),
        WorkflowState::Completed | WorkflowState::Failed
    ) && !pending(&current);
    if matches!(old_state, RunState::Queued | RunState::WaitingForExecutor)
        && stored.record.state == RunState::Running
        && !finish_only
    {
        registry::lock_head(
            tx,
            &stored.record.spec.project_id,
            &current.binding.workflow_id,
        )
        .await?;
        if registry::read_entry(
            tx,
            &stored.record.spec.project_id,
            &current.binding.workflow_id,
        )
        .await?
        .revoked
        {
            return Err(Error::WorkflowRevoked);
        }
    }
    if matches!(old_state, RunState::Queued | RunState::WaitingForExecutor)
        && stored.record.state == RunState::Failed
        && stored.record.error_code.as_deref() == Some("workflow_revoked")
    {
        // A terminal parent start rejection keeps the untouched Ready graph.
        // Verify the exact exception and current revocation in this transaction.
        if !super::completion::revoked_before_start(stored, &current) {
            return Err(Error::Conflict);
        }
        registry::lock_head(
            tx,
            &stored.record.spec.project_id,
            &current.binding.workflow_id,
        )
        .await?;
        if !registry::read_entry(
            tx,
            &stored.record.spec.project_id,
            &current.binding.workflow_id,
        )
        .await?
        .revoked
        {
            return Err(Error::Conflict);
        }
    }
    if *old_state == RunState::Interrupted
        && stored.record.state == RunState::Queued
        && !finish_only
        && change.workflow_resume_operation.is_none()
        && !change.workflow_general_resume
    {
        return Err(Error::Provider("workflow_resume_unavailable".into()));
    }
    if change.workflow_general_resume {
        if *old_state != RunState::Interrupted
            || stored.record.state != RunState::Queued
            || stored.lease.is_some()
            || stored.checkpoint.in_flight.is_some()
        {
            return Err(Error::Forbidden);
        }
        let (completed, cursors) = super::resume::settled_recovery(&current)?;
        let mut expected = current.clone();
        expected.progress.resume_settled_with_tools(
            &expected.plan,
            expected.progress.revision(),
            completed,
            cursors,
            seconds(),
        )?;
        let write = change.workflow_write.as_ref().ok_or(Error::Forbidden)?;
        // Replay the exact recorded decision time; validation also reconstructs the entire history.
        if canonical_hash(&write.next.agents) != canonical_hash(&current.agents)
            || canonical_hash(&write.next.inputs) != canonical_hash(&current.inputs)
            || canonical_hash(write.next.progress.nodes())
                != canonical_hash(expected.progress.nodes())
            || canonical_hash(write.next.progress.attempts())
                != canonical_hash(current.progress.attempts())
            || canonical_hash(&write.next.progress.settled_cursors())
                != canonical_hash(&expected.progress.settled_cursors())
        {
            return Err(Error::Forbidden);
        }
    }
    if let Some(write) = &change.workflow_write {
        if write.new_action {
            registry::lock_head(
                tx,
                &stored.record.spec.project_id,
                &current.binding.workflow_id,
            )
            .await?;
            if registry::read_entry(
                tx,
                &stored.record.spec.project_id,
                &current.binding.workflow_id,
            )
            .await?
            .revoked
            {
                return Err(Error::Forbidden);
            }
        }
        if write.expected_revision != current.progress.revision()
            || write.expected_sha256 != canonical_hash(&current.progress)
            || write.next.progress.revision() != write.expected_revision + 1
            || canonical_hash(&write.next.plan) != canonical_hash(&current.plan)
        {
            return Err(Error::Conflict);
        }
        // The next checkpoint already contains receipts for this transaction's
        // events; event sequence assignment below must agree with this projection.
        let mut validation_record = stored.record.clone();
        validation_record.last_sequence = validation_record
            .last_sequence
            .checked_add(change.events.len() as i64)
            .ok_or(Error::BudgetExceeded)?;
        write.next.validate(&validation_record)?;
    }
    let next = change
        .workflow_write
        .as_ref()
        .map(|w| &w.next)
        .unwrap_or(&current);
    if change.workflow_finish {
        if pending(next) {
            return Err(Error::UncertainEffect);
        }
        let state = next.progress.state();
        if !matches!(state, WorkflowState::Completed | WorkflowState::Failed)
            || stored.record.state != RunState::Running
        {
            return Err(Error::Conflict);
        }
        let content = super::completion::result_content(next)?;
        stored.checkpoint.last_model_content = content.clone();
        let completed = state == WorkflowState::Completed;
        if completed
            && stored
                .record
                .spec
                .folders
                .iter()
                .any(|f| f.order_id.is_some())
        {
            stored.checkpoint.completion_pending = true;
            stored.transition(RunState::WaitingForSnapshot)?;
        } else {
            stored.transition(if completed {
                RunState::Completed
            } else {
                RunState::Failed
            })?;
            stored.record.error_code = if completed {
                None
            } else {
                Some("workflow_failed".into())
            };
            stored.lease = None;
            if !content.is_empty() {
                change.messages.push(Message {
                    id: id(),
                    order_id: stored.record.spec.order_id.clone(),
                    run_id: stored.record.id.clone(),
                    role: "assistant".into(),
                    content,
                    created_at: now(),
                });
            }
        }
        change.events.push(("workflow_finished".into(), json!({"state":stored.record.state,"workflow_revision":next.progress.revision(),"workflow_sha256":canonical_hash(&next.progress),"error_code":stored.record.error_code})));
        if stored.record.state == RunState::Completed {
            change
                .events
                .push(("run_completed".into(), json!({"state":"completed"})));
        }
    }
    if (stored.record.state == RunState::Completed || stored.checkpoint.completion_pending)
        && next.progress.state() != WorkflowState::Completed
    {
        return Err(Error::Conflict);
    }
    if terminal(&stored.record.state) && pending(next) {
        return Err(Error::UncertainEffect);
    }
    if *old_state != RunState::Interrupted
        && stored.record.state == RunState::Interrupted
        && change.workflow_resume_block.is_none()
    {
        let mut recovered = next.clone();
        recovered.progress.recover(
            &recovered.plan,
            recovered.progress.revision(),
            conservative_recovery_at.unwrap_or_else(seconds),
        )?;
        change.events.push((
            "workflow_checkpoint_recovered".into(),
            json!({"revision":recovered.progress.revision(),"automatic_retry":false}),
        ));
        change.workflow_write = Some(WorkflowWrite {
            new_action: false,
            expected_revision: current.progress.revision(),
            expected_sha256: canonical_hash(&current.progress),
            next: recovered,
        });
    }
    if let Some(write) = &change.workflow_write {
        stored.record.steps = write.next.progress.attempts().len() as u32;
        stored.record.model_attempts.clear();
        stored.record.usage = Usage::default();
        for a in write.next.progress.attempts().values() {
            let WorkflowOperation::Model { request_sha256, .. } = &a.operation else {
                continue;
            };
            let usage = match &a.outcome {
                Some(WorkflowOutcome::Measured {
                    input_tokens,
                    output_tokens,
                }) => Usage {
                    input_tokens: Some(*input_tokens),
                    output_tokens: Some(*output_tokens),
                    ..Default::default()
                },
                Some(WorkflowOutcome::Unmeasured {
                    input_tokens,
                    output_tokens,
                }) => Usage {
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                    incomplete: true,
                    ..Default::default()
                },
                _ => Usage {
                    incomplete: true,
                    ..Default::default()
                },
            };
            let usage = agents::reported_usage(&write.next, &a.id, usage);
            let state = match &a.outcome {
                Some(WorkflowOutcome::Measured { .. }) => ModelAttemptState::Measured,
                Some(WorkflowOutcome::Unmeasured { .. } | WorkflowOutcome::Failed) => {
                    ModelAttemptState::Unmeasured
                }
                _ if stored.record.state == RunState::Interrupted => ModelAttemptState::Interrupted,
                _ => ModelAttemptState::InFlight,
            };
            let limits = &write.next.progress.nodes()[&a.claim.node_id].plan.budgets;
            add_reported_usage(&mut stored.record.usage, &usage);
            stored.record.model_attempts.push(ModelAttempt {
                id: a.id.clone(),
                step: a.step,
                request_sha256: Some(request_sha256.clone()),
                reserved_tokens: a.reserved_tokens,
                state,
                started_at: a.started_at.saturating_mul(1000),
                finished_at: a.finished_at.map(|t| t.saturating_mul(1000)),
                limit_exceeded: measured_tokens(&usage) > a.reserved_tokens
                    || usage
                        .output_tokens
                        .is_some_and(|n| n > u64::from(limits.max_output_tokens)),
                usage,
            });
        }
        stored.record.model_attempts.sort_by_key(|a| a.step);
    }
    Ok(())
}
pub(crate) async fn write(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    stored: &StoredRun,
    change: &Change,
) -> Result<()> {
    let Some(write) = &change.workflow_write else {
        return Ok(());
    };
    write.next.validate(&stored.record)?;
    if sqlx::query("UPDATE workflow_runs SET revision=$1,progress_sha256=$2,payload=$3 WHERE run_id=$4 AND project_id=$5 AND revision=$6 AND progress_sha256=$7 AND plan_sha256=$8").bind(write.next.progress.revision() as i64).bind(canonical_hash(&write.next.progress)).bind(json_string(&write.next)?).bind(&stored.record.id).bind(&stored.record.spec.project_id).bind(write.expected_revision as i64).bind(&write.expected_sha256).bind(&write.next.plan.sha256).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
    Ok(())
}
