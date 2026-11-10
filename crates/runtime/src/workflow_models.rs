//! Native per-agent provider adapter. Ordinary worker/API/UI selection remains separate.
use super::*;
use cowork_domain::workflows::WorkflowClaim;
use cowork_storage::{WorkflowModelAdmission, WorkflowModelResponse, WorkflowModelTurn};
impl Runtime {
    /// Starts only the newly committed request. Re-reading a confirmed answer is
    /// safe; a pending duplicate never contacts the provider again.
    pub async fn execute_workflow_model_turn(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        claim: &WorkflowClaim,
        attempt_id: &str,
        stop: &CancellationToken,
    ) -> Result<WorkflowModelTurn> {
        if let Some(turn) = self
            .store
            .workflow_model_turn(project, run, attempt_id)
            .await?
        {
            if turn.claim != *claim {
                return Err(Error::Conflict);
            }
            return if turn.response.is_some() {
                Ok(turn)
            } else {
                Err(Error::UncertainEffect)
            };
        }
        if stop.is_cancelled() {
            return Err(Error::Provider("shutdown".into()));
        }
        let workflow = self.store.stored_workflow(project, run).await?;
        if workflow.is_deterministic_step(&claim.node_id)? {
            let spec = self
                .store
                .workflow_node_spec(project, run, &claim.node_id)
                .await?;
            self.assert_snapshot_controller(&spec).await?;
            return Box::pin(self.store.prepare_workflow_step(
                project,
                run,
                lease,
                claim,
                attempt_id,
                provider::tool_catalog(&spec),
            ))
            .await;
        }
        let spec = self
            .store
            .workflow_node_spec(project, run, &claim.node_id)
            .await?;
        self.assert_snapshot_controller(&spec).await?;
        self.store
            .assert_workflow_clock_authority(project, run, lease)
            .await?;
        let credential = self.workflow_credential(&spec).await?;
        let mut admitted = None;
        let mut conflicts = 0;
        while conflicts < 64 {
            if stop.is_cancelled() {
                return Err(Error::Provider("shutdown".into()));
            }
            let (expected, context) = self
                .store
                .workflow_model_context(project, run, claim, provider::tool_catalog(&spec))
                .await?;
            if canonical_hash(&context.spec) != canonical_hash(&spec)
                || provider::request_fingerprint(&spec, &context.messages)?
                    != context.request_sha256
            {
                return Err(Error::Conflict);
            }
            // Final provider framing, including Ollama argument conversion,
            // precedes this context's durable admission and is never rebuilt.
            let mut prepared = self.prepare_model(&context.spec, &context.messages).await?;
            match self
                .store
                .begin_prepared_workflow_model(
                    project,
                    run,
                    lease,
                    expected,
                    claim,
                    attempt_id,
                    &context,
                    prepared.wire_sha256(),
                )
                .await
            {
                Ok(WorkflowModelAdmission::Dispatch(permit)) => {
                    let turn = permit.into_dispatch();
                    prepared.assert_binding(&turn.context.spec, &turn.context.messages)?;
                    if prepared.logical_fingerprint() != turn.context.request_sha256 {
                        return Err(Error::Conflict);
                    }
                    prepared.bind_dispatch_id(run, &turn.id)?;
                    admitted = Some((turn, prepared));
                    break;
                }
                Ok(WorkflowModelAdmission::Existing(turn)) => {
                    return if turn.response.is_some() {
                        Ok(turn)
                    } else {
                        Err(Error::UncertainEffect)
                    };
                }
                Err(Error::Conflict) => {
                    conflicts += 1;
                    tokio::task::yield_now().await;
                }
                Err(Error::BudgetExceeded) => {
                    // Every continuation reserves its context again. A sibling's
                    // unconfirmed answer may still release transient capacity;
                    // wait before admission, never resend a submitted request.
                    let parent = self.store.stored_run(run).await?;
                    parent.assert_lease(lease, now())?;
                    ensure_running(&parent)?;
                    let graph = self.store.stored_workflow(project, run).await?;
                    if graph.progress.revision() != expected {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    if !transient_workflow_capacity(&graph, claim, now().div_euclid(1000)) {
                        return Err(Error::BudgetExceeded);
                    }
                    self.store.heartbeat(run, lease).await?;
                    tokio::select! {
                        _ = stop.cancelled() => return Err(Error::Provider("shutdown".into())),
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {},
                    }
                }
                Err(error) => return Err(error),
            }
        }
        let (turn, prepared) = admitted.ok_or(Error::Conflict)?;
        // Keep concurrent model turns from embedding the entire provider/store
        // completion state machine in every caller's stack frame.
        let result = Box::pin(self.send_workflow_model(
            project,
            run,
            lease,
            (&turn, prepared),
            credential,
            stop,
        ))
        .await;
        if let Err(error) = &result {
            // No completion/retry after an unconfirmed provider request. A stale
            // parent lease is instead recovered by the common daemon service.
            let _ = self
                .store
                .interrupt_workflow_run(run, lease, error.code())
                .await;
        }
        result
    }
    pub(super) async fn workflow_credential(&self, spec: &RunSpec) -> Result<Option<String>> {
        let frozen = spec.provider.as_ref().ok_or(Error::Forbidden)?;
        provider::validate_profile(frozen, self.allow_test)?;
        let credential = self.personal_provider_credential(frozen).await?;
        if frozen.credential_required && credential.is_none() {
            return Err(Error::Provider("credential_missing".into()));
        }
        Ok(credential)
    }
    async fn send_workflow_model(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        dispatch: (&WorkflowModelTurn, provider::PreparedModelRequest),
        credential: Option<String>,
        stop: &CancellationToken,
    ) -> Result<WorkflowModelTurn> {
        let (turn, prepared) = dispatch;
        self.store
            .assert_workflow_model_dispatch(project, run, lease, turn)
            .await?;
        if provider::tool_catalog(&turn.context.spec) != turn.context.tools
            || provider::request_fingerprint(&turn.context.spec, &turn.context.messages)?
                != turn.context.request_sha256
        {
            return Err(Error::Forbidden);
        }
        prepared.assert_binding(&turn.context.spec, &turn.context.messages)?;
        self.store.registered_model(&turn.context.spec).await?;
        self.assert_snapshot_controller(&turn.context.spec).await?;
        if prepared.logical_fingerprint() != turn.context.request_sha256 {
            return Err(Error::Forbidden);
        }
        // Final native credential/profile check; a missing key never selects another provider.
        if self.workflow_credential(&turn.context.spec).await? != credential {
            return Err(Error::Provider("credential_changed".into()));
        }
        let parent = self.store.stored_run(run).await?;
        let workflow = self.store.stored_workflow(project, run).await?;
        let mut remaining = remaining_run_millis(&parent.record, now());
        let mut node_id = Some(turn.claim.node_id.as_str());
        while let Some(id) = node_id {
            let node = workflow.progress.nodes().get(id).ok_or(Error::Forbidden)?;
            let start = node
                .started_at
                .ok_or(Error::Forbidden)?
                .saturating_mul(1000);
            let elapsed = now()
                .checked_sub(start)
                .filter(|n| *n >= 0)
                .ok_or(Error::BudgetExceeded)? as u64;
            remaining = remaining.min(
                node.plan
                    .budgets
                    .max_seconds
                    .saturating_mul(1000)
                    .saturating_sub(elapsed),
            );
            node_id = node.plan.parent.as_deref();
        }
        if remaining == 0 {
            return Err(Error::BudgetExceeded);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_millis(remaining);
        // Credential and context reads may have awaited a concurrent node/stop
        // change. Recheck this exact turn as well as the fresh parent clock.
        self.store
            .assert_workflow_model_dispatch(project, run, lease, turn)
            .await?;
        let (tx, mut rx) = mpsc::channel(32);
        let money_scope = self
            .store
            .monetary_dispatch_scope(run, lease, &turn.id)
            .await?;
        let receive_observer = provider::ProviderReceiveObserver::new(money_scope);
        let future = provider::complete_prepared_observed(
            prepared,
            credential,
            tx,
            receive_observer.clone(),
        );
        tokio::pin!(future);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        let mut last_heartbeat = tokio::time::Instant::now();
        let receive_result: Result<provider::Reply> = async {
            let reply = loop {
                tokio::select! {
                    biased;
                    _=stop.cancelled()=>{
                        return Err(Error::Provider("shutdown".into()));
                    },
                    _=tokio::time::sleep_until(deadline)=>{
                        return Err(Error::BudgetExceeded);
                    },
                    _=interval.tick()=> {
                        let current=self.store.stored_run(run).await?;
                        current.assert_lease(lease,now())?;
                        if current.record.state!=RunState::Running {
                            return Err(Error::Provider("workflow_canceled_or_interrupted".into()));
                        }
                        if time_budget_exhausted(&current.record,now()) {
                            return Err(Error::BudgetExceeded);
                        }
                        if last_heartbeat.elapsed() >= Duration::from_secs(3) {
                            self.store.heartbeat(run, lease).await?;
                            last_heartbeat = tokio::time::Instant::now();
                        }
                    },
                    result=&mut future=>break result?,
                    Some(delta)=rx.recv()=>self.workflow_delta(run,lease,turn,&delta).await?,
                }
            };
            while let Ok(delta) = rx.try_recv() {
                self.workflow_delta(run, lease, turn, &delta).await?;
            }
            Ok(reply)
        }
        .await;
        let reply = match receive_result {
            Ok(reply) => reply,
            Err(error) => {
                self.note_provider_uncertainty(run, &turn.id, &receive_observer)
                    .await?;
                return Err(error);
            }
        };
        let response = WorkflowModelResponse {
            provider_continuation: reply.provider_continuation.clone(),
            received_cost_metadata: reply.received_cost_metadata.clone(),
            rejection_code: if reply.tools.is_empty()
                && !cowork_domain::hybrid_planning(&turn.context.spec, &turn.context.messages)
                && turn.context.spec.output.as_ref().is_some_and(|output| {
                    cowork_domain::structured_output::validate_result(output, &reply.content)
                        .is_err()
                }) {
                Some("structured_output_mismatch".into())
            } else {
                None
            },
            content: reply.content,
            tools: reply.tools,
            usage: reply.usage,
        };
        // Concurrent agents may have advanced the shared revision. Retry only the
        // received answer's transaction, never the provider request.
        for _ in 0..64 {
            match self
                .store
                .finish_workflow_model_received(
                    project,
                    run,
                    lease,
                    &turn.claim,
                    &turn.id,
                    response.clone(),
                    reply.received.as_ref(),
                )
                .await
            {
                Ok(turn) => return Ok(turn),
                Err(Error::Conflict) => tokio::task::yield_now().await,
                Err(error) => {
                    self.note_provider_uncertainty(run, &turn.id, &receive_observer)
                        .await?;
                    return Err(error);
                }
            }
        }
        self.note_provider_uncertainty(run, &turn.id, &receive_observer)
            .await?;
        Err(Error::Conflict)
    }
    async fn workflow_delta(
        &self,
        run: &str,
        lease: &str,
        turn: &WorkflowModelTurn,
        delta: &str,
    ) -> Result<()> {
        self.store
            .mutate(run, Some(lease), |r| {
                if !matches!(
                    r.record.state,
                    RunState::Running | RunState::CancelRequested
                ) {
                    return Err(Error::Conflict);
                }
                Ok(Change::event(
                    "workflow_model_delta",
                    json!({"node_id":turn.claim.node_id,"attempt_id":turn.id,"text":delta}),
                ))
            })
            .await?;
        Ok(())
    }
}

/// Waiting is useful only if an unconfirmed sibling reservation can release the
/// exact exhausted token pool. Step, elapsed-time and final usage cannot recover.
fn transient_workflow_capacity(
    graph: &cowork_storage::StoredWorkflow,
    claim: &WorkflowClaim,
    at: i64,
) -> bool {
    let nodes = graph.progress.nodes();
    let attempts = graph.progress.attempts();
    let descendant = |node_id: &str, ancestor: &str| -> bool {
        let mut id = node_id.to_owned();
        loop {
            if id == ancestor {
                return true;
            }
            match nodes[&id].plan.parent.as_deref() {
                Some(parent) => id = parent.to_owned(),
                None => return false,
            }
        }
    };
    let pending = |a: &&cowork_domain::workflows::WorkflowAttempt| {
        a.claim.node_id != claim.node_id
            && a.outcome.is_none()
            && matches!(
                a.operation,
                cowork_domain::workflows::WorkflowOperation::Model { .. }
            )
    };
    let Some(role_id) = nodes[&claim.node_id].plan.role_id.as_deref() else {
        return false;
    };
    let needed = u64::from(graph.plan.roles[role_id].provider.context_tokens);
    if attempts.len() >= graph.plan.budgets.max_steps as usize {
        return false;
    }
    let mut deficit = graph.progress.charged_tokens().saturating_add(needed)
        > graph.plan.budgets.max_total_tokens;
    if deficit && !attempts.values().any(|a| pending(&a)) {
        return false;
    }
    let mut id = Some(claim.node_id.as_str());
    while let Some(ancestor) = id {
        let node = &nodes[ancestor];
        if node
            .started_at
            .is_some_and(|start| at < start || at - start >= node.plan.budgets.max_seconds as i64)
        {
            return false;
        }
        let own: Vec<_> = attempts
            .values()
            .filter(|a| descendant(&a.claim.node_id, ancestor))
            .collect();
        if own.len() >= node.plan.budgets.max_steps as usize {
            return false;
        }
        let charged = own
            .iter()
            .map(|a| a.charged_tokens())
            .fold(0u64, u64::saturating_add);
        if charged.saturating_add(needed) > node.plan.budgets.max_total_tokens {
            deficit = true;
            if !own.iter().any(pending) {
                return false;
            }
        }
        id = node.plan.parent.as_deref();
    }
    deficit
}
