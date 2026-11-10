//! Product-safe workflow entry points and a derived durable attention index.
//! The index never owns run state or grants execution rights.
use super::*;
use cowork_workflow_contracts::{
    WorkflowModelView, WorkflowNodeKind, WorkflowNodeStatus, WorkflowNodeView, WorkflowPage,
    WorkflowPublicState, WorkflowRunApproval, WorkflowRunRequest, WorkflowRunView,
};
use std::collections::{BTreeMap, BTreeSet};

const PRODUCT_PARALLELISM: u16 = 4;
pub(super) const PRODUCT_TOOLS: [&str; 4] = [
    "create_artifact",
    "list_artifacts",
    "read_artifact",
    "request_input",
];
pub(super) const NATIVE_TOOLS: [&str; 4] = ["execute_program", "browser", "office", "github"];
pub(super) fn valid_product_tools(tools: &[String]) -> bool {
    let set: BTreeSet<_> = tools.iter().map(String::as_str).collect();
    set.len() == tools.len()
        && set.iter().all(|name| {
            PRODUCT_TOOLS.contains(name)
                || NATIVE_TOOLS.contains(name)
                || matches!(
                    *name,
                    "delegate_agents"
                        | "list_files"
                        | "read_text_file"
                        | "stat_file"
                        | "write_text_file"
                        | "create_directory"
                        | "rename_file"
                        | "remove_file"
                        | "restore_file"
                        | "document_read"
                        | "document_write"
                        | "image_edit"
                )
                || name.starts_with("mcp_")
        })
}
pub(crate) fn request_hash(input: &WorkflowRunRequest) -> String {
    canonical_hash(&json!({"namespace":"cowork-product-workflow-v1","request":input}))
}
pub(crate) fn start(
    input: &WorkflowRunRequest,
    expected: Option<WorkflowBinding>,
) -> WorkflowStart {
    WorkflowStart {
        inputs: input.inputs.clone(),
        workflow_id: input.workflow_id.clone(),
        version_id: input.version_id.clone(),
        max_parallel: PRODUCT_PARALLELISM,
        product: true,
        expected_binding: expected,
    }
}
pub(crate) fn validate_scope(input: &WorkflowRunRequest, spec: &RunSpec) -> Result<()> {
    valid_text(&input.idempotency_key, 128)?;
    valid_text(&input.prompt, 64_000)?;
    valid_text(&input.workflow_id, 128)?;
    valid_text(&input.version_id, 128)?;
    validate_budgets(&input.budgets)?;
    let tools: BTreeSet<_> = spec.allowed_tools.iter().map(String::as_str).collect();
    let mut expected: BTreeSet<String> = PRODUCT_TOOLS
        .into_iter()
        .chain(input.native_tools.iter().map(String::as_str))
        .map(str::to_owned)
        .collect();
    if !spec.folders.is_empty() {
        expected.extend(["list_files", "read_text_file", "stat_file"].map(str::to_owned));
    }
    if spec.folders.iter().any(|f| f.writable) {
        expected.extend(
            [
                "write_text_file",
                "create_directory",
                "rename_file",
                "remove_file",
                "restore_file",
            ]
            .map(str::to_owned),
        );
    }
    if tools.iter().any(|tool| documents::document_tool(tool)) {
        if spec.folders.is_empty() || spec.environment.is_some() {
            return Err(Error::Forbidden);
        }
        expected.insert("document_read".into());
        if spec.folders.iter().any(|folder| folder.writable) {
            expected.extend(["document_write".into(), "image_edit".into()]);
        }
    }
    if input.environment.is_some() {
        expected.insert("execute_program".into());
    }
    for tool in &input.mcp_tools {
        expected.insert(cowork_domain::mcp::mcp_alias(
            &tool.server_id,
            &tool.tool_name,
        ));
    }
    if tools.contains("delegate_agents") {
        expected.insert("delegate_agents".into());
    }
    if let Some(config) = &input.agent_config {
        cowork_domain::validate_agent_values(config)?;
        if let Some(filter) = &config.tool_filter {
            expected.retain(|tool| filter.contains(tool));
        }
    }
    if matches!(input.access_mode, AccessMode::ReadOnly) {
        expected.retain(|tool| cowork_domain::read_only_tool(tool));
    }

    if spec.provider.is_some()
        || spec.workflow.is_some()
        || spec.file_action.is_some()
        || spec.order_id != input.order_id
        || spec.prompt != input.prompt
        || spec.access_mode != input.access_mode
        || spec.budgets != input.budgets
        || (spec.environment.is_none()
            && spec.executor
                != if tools.iter().any(|tool| NATIVE_TOOLS.contains(tool)) {
                    "owner-native-toolbox-v1"
                } else {
                    "local-v2"
                })
        || spec.policy_revision != 1
        || !valid_product_tools(&spec.allowed_tools)
        || tools != expected.iter().map(String::as_str).collect::<BTreeSet<_>>()
        || input.native_tools.len() != input.native_tools.iter().collect::<BTreeSet<_>>().len()
        || input
            .native_tools
            .iter()
            .any(|name| !NATIVE_TOOLS.contains(&name.as_str()))
        || tools.len() != spec.allowed_tools.len()
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
pub(super) fn validate_definition(definition: &WorkflowDefinition) -> Result<()> {
    let allowed: BTreeSet<_> = PRODUCT_TOOLS
        .into_iter()
        .chain(NATIVE_TOOLS)
        .map(str::to_owned)
        .collect();
    let valid = |rights: &WorkflowRights| {
        rights.tools.iter().all(|name| {
            allowed.contains(name)
                || matches!(
                    name.as_str(),
                    "delegate_agents"
                        | "list_files"
                        | "read_text_file"
                        | "stat_file"
                        | "write_text_file"
                        | "create_directory"
                        | "rename_file"
                        | "remove_file"
                        | "restore_file"
                        | "document_read"
                        | "document_write"
                        | "image_edit"
                )
                || name.starts_with("mcp_")
        })
    };
    if !valid(&definition.rights)
        || definition.roles.iter().any(|r| !valid(&r.rights))
        || definition.steps.iter().any(|s| !valid(&s.rights))
        || definition
            .subflows
            .values()
            .any(|call| !valid(&call.rights))
        || definition
            .iterations
            .values()
            .any(|call| !valid(&call.rights))
    {
        return Err(Error::Validation("workflow_capability_unavailable".into()));
    }
    if !definition.subflows.is_empty() {
        definition.validate_definition()?;
        for call in definition.subflows.values() {
            validate_definition(&call.definition)?;
        }
    }
    if !definition.iterations.is_empty() {
        definition.validate_definition()?;
        for call in definition.iterations.values() {
            validate_definition(&call.definition)?;
        }
    }
    Ok(())
}
impl Store {
    /// Original public request identity, independent of derived profiles/scopes.
    pub async fn existing_product_workflow_request(
        &self,
        input: &WorkflowRunRequest,
    ) -> Result<Option<RunRecord>> {
        self.existing_run_request(
            &input.order_id,
            &input.idempotency_key,
            &request_hash(input),
        )
        .await
    }
    pub async fn create_product_workflow_run(
        &self,
        input: &WorkflowRunRequest,
        spec: RunSpec,
        expected: &WorkflowBinding,
    ) -> Result<RunRecord> {
        if let Some(existing) = self.existing_product_workflow_request(input).await? {
            return Ok(existing);
        }
        validate_scope(input, &spec)?;
        if input
            .expected_binding
            .as_ref()
            .is_some_and(|binding| canonical_hash(binding) != canonical_hash(expected))
        {
            return Err(Error::Conflict);
        }
        let request = RunRequest {
            workflow: Some(start(input, Some(expected.clone()))),
            order_id: input.order_id.clone(),
            base_revision: input.base_revision,
            idempotency_key: input.idempotency_key.clone(),
            prompt: input.prompt.clone(),
        };
        let identity = crate::workflow_operations::action_identity(
            &spec.project_id,
            &cowork_workflow_contracts::WorkflowOperationRequest::Start {
                request: input.clone().into(),
            },
        );
        self.insert_run(&request, spec, request_hash(input), false, identity)
            .await
    }
    /// Reuses transactional compilation without publishing a run or a dispatch permit.
    pub async fn product_preflight_compilation(
        &self,
        input: &WorkflowRunRequest,
        spec: &RunSpec,
    ) -> Result<WorkflowPlan> {
        validate_scope(input, spec)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::workspace_management::order_lock(&mut tx, &input.order_id).await?;
        let payload: Option<String> = sqlx::query_scalar("SELECT payload FROM orders WHERE id=$1")
            .bind(&input.order_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let order: Order = parse(&payload.ok_or(Error::NotFound)?)?;
        if order.project_id != spec.project_id || order.revision != input.base_revision {
            return Err(Error::Conflict);
        }
        let mut scoped = spec.clone();
        let plan = runs::initialize(
            &mut tx,
            &mut scoped,
            &start(input, input.expected_binding.clone()),
        )
        .await?;
        tx.rollback().await.map_err(storage_error)?;
        Ok(plan)
    }
    /// Exact ordinary seed + shared agent assembly for initially runnable nodes only.
    /// Dependencies with future outputs are checked by the real runtime on admission.
    pub async fn preview_product_workflow_contexts(
        &self,
        input: &WorkflowRunRequest,
        spec: &RunSpec,
        plan: &WorkflowPlan,
        tools_by_node: &BTreeMap<String, Value>,
    ) -> Result<Vec<WorkflowModelContext>> {
        validate_scope(input, spec)?;
        validate_definition(&plan.definition)?;
        plan.validate()?;
        if plan.scope.max_parallel != PRODUCT_PARALLELISM
            || plan.scope.budgets != spec.budgets
            || plan.scope.rights.tools != spec.allowed_tools.iter().cloned().collect()
            || plan.scope.rights.folders != spec.folders.iter().map(|f| f.id.clone()).collect()
        {
            return Err(Error::Forbidden);
        }
        let mut tx = self.snapshot_transaction().await?;
        let mut seed = model_context(&mut tx, spec).await?;
        crate::workflow_role_contexts::freeze(&mut tx, spec, plan, &mut seed).await?;
        let mut frozen = spec.clone();
        frozen.budgets = plan.budgets.clone();
        frozen.workflow = Some(WorkflowBinding {
            inputs: input.inputs.clone(),
            workflow_id: input.workflow_id.clone(),
            version_id: input.version_id.clone(),
            definition_sha256: plan.source_sha256.clone(),
            plan_sha256: plan.sha256.clone(),
        });
        let at = now();
        let parent = StoredRun {
            record: RunRecord {
                id: "preflight-workflow".into(),
                spec: frozen,
                state: RunState::Queued,
                revision: 1,
                last_sequence: 0,
                steps: 0,
                created_at: at,
                updated_at: at,
                started_at: None,
                error_code: None,
                usage: Usage::default(),
                model_attempts: vec![],
            },
            checkpoint: Checkpoint {
                messages: seed,
                ..Default::default()
            },
            lease: None,
        };
        let workflow = StoredWorkflow {
            format_version: 1,
            run_id: parent.record.id.clone(),
            project_id: spec.project_id.clone(),
            binding: parent
                .record
                .spec
                .workflow
                .clone()
                .ok_or(Error::Forbidden)?,
            plan: plan.clone(),
            progress: WorkflowProgress::new(plan)?,
            agents: BTreeMap::new(),
            inputs: BTreeMap::new(),
            samplings: BTreeMap::new(),
        };
        let ready = workflow.progress.ready_nodes(plan)?;
        let mut contexts = Vec::new();
        for node in ready {
            if plan.nodes[&node].role_id.is_none() {
                continue;
            }
            let mut initial = workflow.clone();
            let claim =
                initial
                    .progress
                    .claim(plan, initial.progress.revision(), &node, seconds())?;
            let tools = tools_by_node.get(&node).ok_or(Error::Forbidden)?.clone();
            contexts.push(agents::build_context(&parent, &initial, &claim, tools)?);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(contexts)
    }
    pub async fn list_workflows(
        &self,
        project: &str,
        offset: u32,
        limit: u32,
    ) -> Result<WorkflowPage> {
        page(offset, limit)?;
        let mut tx = self.snapshot_transaction().await?;
        if sqlx::query_scalar::<_, String>("SELECT id FROM projects WHERE id=$1")
            .bind(project)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .is_none()
        {
            return Err(Error::NotFound);
        }
        let total =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflows WHERE project_id=$1")
                .bind(project)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        let ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM workflows WHERE project_id=$1 ORDER BY id LIMIT $2 OFFSET $3",
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
            items.push(registry::read_entry(&mut tx, project, &id).await?);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(WorkflowPage {
            items,
            total,
            next_offset,
        })
    }
    pub async fn workflow_run_view(&self, project: &str, run: &str) -> Result<WorkflowRunView> {
        for attempt in 0..12 {
            match self.workflow_run_view_once(project, run).await {
                Err(error) if retryable_projection(&error) && attempt < 11 => {
                    tokio::time::sleep(Duration::from_millis(5 * (attempt + 1))).await;
                }
                result => return result,
            }
        }
        Err(Error::Conflict)
    }
    async fn workflow_run_view_once(&self, project: &str, run: &str) -> Result<WorkflowRunView> {
        let mut tx = self.snapshot_transaction().await?;
        let payload: Option<String> = sqlx::query_scalar("SELECT payload FROM runs WHERE id=$1")
            .bind(run)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let parent: StoredRun = parse(&payload.ok_or(Error::NotFound)?)?;
        if parent.record.spec.project_id != project {
            return Err(Error::NotFound);
        }
        let workflow = runs::load(&mut tx, &parent.record).await?;
        let version = registry::read_version(
            &mut tx,
            project,
            &workflow.binding.workflow_id,
            &workflow.binding.version_id,
        )
        .await?;
        let mut approvals = refresh_attention(&mut tx, &parent).await?;
        let revoked = registry::read_entry(&mut tx, project, &workflow.binding.workflow_id)
            .await?
            .revoked;
        // Read-only actionability must not create an origin or refresh the
        // clock. Internal attention refreshes run before creation/CAS finishes,
        // so this gate belongs only to the committed product snapshot read.
        let (execution_at, clock_block) = match self
            .workflow_clock
            .execution_now(&mut tx, &parent.record)
            .await
        {
            Ok(at) => (Some(at), None),
            Err(Error::WorkflowClockUnverified) => (None, Some("workflow_clock_unverified")),
            Err(Error::WorkflowClockRollback) => (None, Some("workflow_clock_rollback")),
            Err(error) => return Err(error),
        };
        let mut inputs = inputs::views(&parent, &workflow, revoked)?;
        if let Some(reason) = clock_block {
            approvals.clear();
            for input in &mut inputs {
                if input.actionable {
                    input.actionable = false;
                    input.state = cowork_workflow_contracts::WorkflowInputState::Closed;
                    input.closed_reason = Some(reason.into());
                }
            }
        }
        if parent.record.state == RunState::WaitingForTimer {
            super::timers::validate_projection(&mut tx, &workflow).await?;
        }
        let ready: BTreeSet<_> = if matches!(
            parent.record.state,
            RunState::Queued | RunState::WaitingForExecutor | RunState::Running
        ) {
            workflow
                .progress
                .ready_nodes(&workflow.plan)?
                .into_iter()
                .collect()
        } else {
            BTreeSet::new()
        };
        let scopes = workflow.execution_scopes()?;
        let mut nodes: Vec<WorkflowNodeView> = workflow
            .progress
            .nodes()
            .iter()
            .map(|(id, node)| {
                let (mut state, error_code) = match &node.state {
                    WorkflowNodeState::Pending => (
                        if ready.contains(id) {
                            WorkflowNodeStatus::Ready
                        } else {
                            WorkflowNodeStatus::Pending
                        },
                        None,
                    ),
                    WorkflowNodeState::Running { .. } | WorkflowNodeState::WaitingChildren => {
                        (WorkflowNodeStatus::Running, None)
                    }
                    WorkflowNodeState::WaitingTimer { .. } => (
                        if terminal(&parent.record.state) {
                            WorkflowNodeStatus::Blocked
                        } else {
                            WorkflowNodeStatus::WaitingTimer
                        },
                        None,
                    ),
                    WorkflowNodeState::Skipped { .. } => (WorkflowNodeStatus::Skipped, None),
                    WorkflowNodeState::Completed { .. } => (WorkflowNodeStatus::Completed, None),
                    WorkflowNodeState::Failed { code } => {
                        (WorkflowNodeStatus::Failed, Some(code.clone()))
                    }
                    WorkflowNodeState::Blocked => (WorkflowNodeStatus::Blocked, None),
                    WorkflowNodeState::Interrupted => (WorkflowNodeStatus::Interrupted, None),
                };
                if matches!(
                    parent.record.state,
                    RunState::Interrupted
                        | RunState::Canceled
                        | RunState::Failed
                        | RunState::Expired
                ) && matches!(
                    node.state,
                    WorkflowNodeState::Running { .. } | WorkflowNodeState::WaitingChildren
                ) {
                    state = WorkflowNodeStatus::Interrupted;
                }
                if inputs.iter().any(|q| q.node_id == *id && q.actionable) {
                    state = WorkflowNodeStatus::WaitingInput;
                }
                if approvals.iter().any(|a| a.node_id == *id) {
                    state = WorkflowNodeStatus::WaitingApproval;
                }
                let attempts = workflow
                    .progress
                    .attempts()
                    .values()
                    .filter(|a| a.claim.node_id == *id)
                    .collect::<Vec<_>>();
                WorkflowNodeView {
                    kind: if scopes.contains_key(id) {
                        Some(WorkflowNodeKind::Subflow)
                    } else if scopes.values().any(|scope| scope.entry_node == *id) {
                        Some(WorkflowNodeKind::Arguments)
                    } else {
                        None
                    },
                    scope_id: node
                        .plan
                        .parent
                        .as_ref()
                        .filter(|parent| scopes.contains_key(*parent))
                        .cloned(),
                    id: id.clone(),
                    role_id: node.plan.role_id.clone(),
                    wake_at: if !terminal(&parent.record.state) {
                        if let WorkflowNodeState::WaitingTimer { wake_at, .. } = &node.state {
                            Some(*wake_at)
                        } else {
                            None
                        }
                    } else {
                        None
                    },
                    skip_reason: if let WorkflowNodeState::Skipped { reason } = &node.state {
                        Some(reason.clone())
                    } else {
                        None
                    },
                    goal: node.plan.goal.clone(),
                    dependencies: node.plan.dependencies.iter().cloned().collect(),
                    state,
                    error_code,
                    steps: attempts.len() as u32,
                    charged_tokens: attempts.iter().map(|a| a.charged_tokens()).sum(),
                }
            })
            .collect();
        drop(scopes);
        super::presentation::iteration_order(&mut nodes, workflow.progress.iteration_manifests());
        super::presentation::composition_status(&mut nodes);
        let state = match parent.record.state {
            RunState::Queued | RunState::WaitingForExecutor => WorkflowPublicState::Queued,
            RunState::WaitingForTimer => WorkflowPublicState::WaitingTimer,
            RunState::Interrupted => WorkflowPublicState::Interrupted,
            RunState::Completed => WorkflowPublicState::Completed,
            RunState::Failed | RunState::Expired => WorkflowPublicState::Failed,
            RunState::Canceled => WorkflowPublicState::Canceled,
            _ => WorkflowPublicState::Running,
        };
        let completion_resume_available = parent.record.state == RunState::Interrupted
            && parent.checkpoint.in_flight.is_none()
            && execution_at.is_some_and(|at| !time_budget_exhausted(&parent.record, at))
            && matches!(
                workflow.progress.state(),
                WorkflowState::Completed | WorkflowState::Failed
            )
            && workflow
                .progress
                .attempts()
                .values()
                .all(|a| a.outcome.is_some() && a.outcome != Some(WorkflowOutcome::Uncertain));
        let input_resume_preview =
            super::continuations::preview(self, &mut tx, &parent, &workflow).await?;
        let input_resume_blocked_reason = input_resume_preview.blocked_reason.clone();
        let settled = super::resume::settled_outputs(&workflow);
        let settled_resume_available = parent.record.state == RunState::Interrupted
            && parent.checkpoint.in_flight.is_none()
            && execution_at.is_some_and(|at| !time_budget_exhausted(&parent.record, at))
            && settled.is_ok();
        let settled_resume_blocked_reason = settled
            .err()
            .map(|_| "workflow_effect_evidence_required".to_owned());
        let result = WorkflowRunView {
            run: parent.record,
            binding: workflow.binding,
            name: version.reference.name,
            version_revision: version.reference.revision,
            mode: workflow.plan.definition.mode.clone(),
            max_parallel: workflow.plan.max_parallel,
            state,
            roles: workflow
                .plan
                .roles
                .values()
                .map(|r| WorkflowModelView {
                    role_id: r.id.clone(),
                    provider: r.provider.clone(),
                })
                .collect(),
            nodes,
            approvals,
            inputs,
            input_resume_available: input_resume_preview.available,
            input_resume_preview,
            input_resume_blocked_reason,
            completion_resume_available,
            settled_resume_available,
            settled_resume_blocked_reason,
            charged_tokens: workflow.progress.charged_tokens(),
            steps_used: workflow.progress.attempts().len() as u32,
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
fn page(offset: u32, limit: u32) -> Result<()> {
    if limit == 0 || limit > 100 || offset > 1_000_000 {
        return Err(Error::Validation("Invalid pagination".into()));
    }
    Ok(())
}
pub(crate) async fn refresh_attention(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &StoredRun,
) -> Result<Vec<WorkflowRunApproval>> {
    if parent.record.spec.workflow.is_none() {
        return Ok(vec![]);
    }
    let workflow = runs::load(tx, &parent.record).await?;
    let revoked = registry::read_entry(tx, &workflow.project_id, &workflow.binding.workflow_id)
        .await?
        .revoked;
    let actionable = tools::actionable_approvals(parent, &workflow, revoked)?;
    let question_views = inputs::views(parent, &workflow, revoked)?;
    let question_deadlines = question_views.iter().filter(|q| q.actionable).map(|q| {
        q.expires_at
            .min(parent.lease.as_ref().map_or(0, |l| l.until))
    });
    let until = actionable
        .iter()
        .map(|(_, deadline)| *deadline)
        .chain(question_deadlines)
        .min();
    let actionable_count =
        actionable.len() + question_views.iter().filter(|q| q.actionable).count();
    sqlx::query("INSERT INTO workflow_attention(run_id,run_revision,workflow_revision,actionable_count,valid_until) VALUES($1,$2,$3,$4,$5) ON CONFLICT(run_id) DO UPDATE SET run_revision=excluded.run_revision,workflow_revision=excluded.workflow_revision,actionable_count=excluded.actionable_count,valid_until=excluded.valid_until WHERE workflow_attention.run_revision<>excluded.run_revision OR workflow_attention.workflow_revision<>excluded.workflow_revision OR workflow_attention.actionable_count<>excluded.actionable_count OR COALESCE(workflow_attention.valid_until,0)<>COALESCE(excluded.valid_until,0)")
        .bind(&parent.record.id).bind(parent.record.revision).bind(workflow.progress.revision() as i64)
        .bind(actionable_count as i64).bind(until).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(actionable.into_iter().map(|(a, _)| a).collect())
}
pub(crate) async fn refresh_all_attention(tx: &mut sqlx::Transaction<'_, sqlx::Any>) -> Result<()> {
    let rows=sqlx::query("SELECT r.payload FROM runs r JOIN workflow_runs w ON w.run_id=r.id LEFT JOIN workflow_attention a ON a.run_id=r.id WHERE a.run_id IS NULL OR a.actionable_count>0 ORDER BY r.id")
        .fetch_all(&mut **tx).await.map_err(storage_error)?;
    for row in rows {
        let parent: StoredRun =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        refresh_attention(tx, &parent).await?;
    }
    Ok(())
}

pub(crate) fn retryable_projection(error: &Error) -> bool {
    let Error::Storage(source) = error else {
        return false;
    };
    let Some(sql) = source.downcast_ref::<sqlx::Error>() else {
        return false;
    };
    is_busy(sql)
        || matches!(sql,sqlx::Error::Database(database) if database.code().as_deref().is_some_and(|code|code=="40001" || code=="40P01"))
}
