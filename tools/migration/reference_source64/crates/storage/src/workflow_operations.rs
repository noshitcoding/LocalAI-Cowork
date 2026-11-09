//! Immutable pending intents; acceptance is committed with the actual effect.
use super::*;
use cowork_workflow_contracts::*;
use workflow_drafts::{page, require_project, reserve_quota};

pub(super) fn action_identity(project: &str, request: &WorkflowOperationRequest) -> Value {
    json!({"namespace":"cowork-workflow-operation-v1","project_id":project,"request":request})
}
fn operation_digest(
    project: &str,
    request: &WorkflowOperationRequest,
    snapshot: &Option<WorkflowDraftSnapshotRef>,
) -> String {
    canonical_hash(&json!({"project_id":project,"request":request,"draft_snapshot":snapshot}))
}
fn coordinates<'a>(
    project: &'a str,
    request: &'a WorkflowOperationRequest,
) -> (&'static str, &'a str, &'a str) {
    match request {
        WorkflowOperationRequest::Create { request } => {
            ("workflow-create", project, &request.idempotency_key)
        }
        WorkflowOperationRequest::Publish {
            workflow_id,
            request,
        } => ("workflow-change", workflow_id, &request.idempotency_key),
        WorkflowOperationRequest::Revoke {
            workflow_id,
            request,
        } => ("workflow-change", workflow_id, &request.idempotency_key),
        WorkflowOperationRequest::Start { request } => {
            ("run", &request.order_id, &request.idempotency_key)
        }
        WorkflowOperationRequest::ResumeInput { request } => (
            "workflow-input-resume",
            &request.run_id,
            &request.idempotency_key,
        ),
    }
}
impl Store {
    pub async fn commit_workflow_input_resume_operation(
        &self,
        project: &str,
        id: &str,
    ) -> Result<WorkflowOperation> {
        let operation = self.workflow_operation(project, id).await?;
        if operation.state != WorkflowOperationState::Pending {
            return Ok(operation);
        }
        let WorkflowOperationRequest::ResumeInput { request } = &operation.request else {
            return Err(Error::Forbidden);
        };
        self.mutate_with_journal(
            &request.run_id,
            None,
            crate::workflow_clocks::WorkflowClockIntent::Execution,
            Some(&operation),
            None,
            |_| Ok(Change::default()),
        )
        .await?;
        self.workflow_operation(project, id).await
    }
    pub async fn prepare_workflow_operation(
        &self,
        project: &str,
        input: &PrepareWorkflowOperation,
    ) -> Result<WorkflowOperation> {
        valid_text(&input.intent_id, 128)?;
        if matches!(&input.request, WorkflowOperationRequest::ResumeInput { .. })
            && input.draft_snapshot.is_some()
        {
            return Err(Error::Forbidden);
        }
        if json_string(input)?.len() > 1_000_000 {
            return Err(Error::Validation("Operation exceeds one megabyte".into()));
        }
        let (namespace, target, key) = coordinates(project, &input.request);
        let digest = operation_digest(project, &input.request, &input.draft_snapshot);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        require_project(&mut tx, project).await?;
        let reservation = workflow_write_keys::lock(
            &mut tx,
            namespace,
            target,
            key,
            project,
            &action_identity(project, &input.request),
        )
        .await?;
        if let Some(owner) = &reservation.operation_id {
            let existing = read_operation(&mut tx, project, owner, true).await?;
            if existing.request_sha256 != digest || existing.intent_id != input.intent_id {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(existing);
        }
        if matches!(reservation.state.as_str(), "discarded" | "rejected") {
            return Err(Error::Conflict);
        }
        if reservation.state == "reserved" && legacy_effect(&mut tx, namespace, target, key).await?
        {
            // A pre-journal receipt cannot prove every original field. Do not invent it.
            return Err(Error::Conflict);
        }
        let at = now();
        let receipt = reservation
            .receipt
            .map(|v| serde_json::from_value::<WorkflowOperationReceipt>(v).map_err(storage_error))
            .transpose()?;
        if reservation.state == "accepted" && receipt.is_none() {
            return Err(Error::Forbidden);
        }
        let candidate = WorkflowOperation {
            id: id(),
            project_id: project.into(),
            intent_id: input.intent_id.clone(),
            request_sha256: digest.clone(),
            request: input.request.clone(),
            draft_snapshot: input.draft_snapshot.clone(),
            state: if receipt.is_some() {
                WorkflowOperationState::Accepted
            } else {
                WorkflowOperationState::Pending
            },
            receipt,
            rejection: None,
            created_at: at,
            updated_at: at,
            acknowledged_at: None,
        };
        if let Some(reference) = &input.draft_snapshot {
            let snapshot = workflow_drafts::read_snapshot(
                &mut tx,
                project,
                &reference.draft_id,
                &reference.snapshot_id,
            )
            .await?;
            if snapshot.sha256 != reference.sha256 {
                return Err(Error::Conflict);
            }
        }
        let inserted=sqlx::query("INSERT INTO workflow_operations(id,project_id,intent_key,request_sha256,state,payload) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(project_id,intent_key) DO NOTHING")
            .bind(&candidate.id).bind(project).bind(&input.intent_id).bind(&digest).bind(state_name(&candidate.state)).bind(json_string(&candidate)?)
            .execute(&mut *tx).await.map_err(storage_error)?.rows_affected()==1;
        let existing_id = sqlx::query_scalar::<_, String>(
            "SELECT id FROM workflow_operations WHERE project_id=$1 AND intent_key=$2",
        )
        .bind(project)
        .bind(&input.intent_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let operation = read_operation(&mut tx, project, &existing_id, true).await?;
        if operation.request_sha256 != digest {
            return Err(Error::Conflict);
        }
        if inserted {
            check_target(&mut tx, project, &input.request).await?;
            reserve_quota(&mut tx, project, 0, 0, 0, 1).await?;
        }
        if sqlx::query("UPDATE workflow_write_keys SET operation_id=$1 WHERE namespace=$2 AND target_id=$3 AND request_key=$4 AND operation_id IS NULL")
            .bind(&operation.id).bind(namespace).bind(target).bind(key).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1 { return Err(Error::Conflict); }
        tx.commit().await.map_err(storage_error)?;
        Ok(operation)
    }
    pub async fn workflow_operation(
        &self,
        project: &str,
        operation: &str,
    ) -> Result<WorkflowOperation> {
        let mut tx = self.snapshot_transaction().await?;
        let result = read_operation(&mut tx, project, operation, false).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn list_workflow_operations(
        &self,
        project: &str,
        offset: u32,
        limit: u32,
        include_acknowledged: bool,
    ) -> Result<WorkflowOperationPage> {
        page(offset, limit)?;
        let mut tx = self.snapshot_transaction().await?;
        require_project(&mut tx, project).await?;
        let total=sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM workflow_operations WHERE project_id=$1 AND ($2=1 OR acknowledged_at IS NULL)")
            .bind(project).bind(i64::from(include_acknowledged)).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let ids=sqlx::query_scalar::<_,String>("SELECT id FROM workflow_operations WHERE project_id=$1 AND ($2=1 OR acknowledged_at IS NULL) ORDER BY id LIMIT $3 OFFSET $4")
            .bind(project).bind(i64::from(include_acknowledged)).bind(i64::from(limit)+1).bind(i64::from(offset)).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let next_offset = (ids.len() > limit as usize).then_some(offset + limit);
        let mut items = Vec::new();
        for id in ids.into_iter().take(limit as usize) {
            items.push(read_operation(&mut tx, project, &id, false).await?);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(WorkflowOperationPage {
            items,
            total,
            next_offset,
        })
    }
    pub async fn discard_workflow_operation(
        &self,
        project: &str,
        operation: &str,
    ) -> Result<WorkflowOperation> {
        self.finish_pending_workflow_operation(project, operation, None)
            .await
    }
    pub async fn reject_pending_workflow_operation(
        &self,
        project: &str,
        operation: &str,
        rejection: &WorkflowOperationRejection,
    ) -> Result<WorkflowOperation> {
        if !matches!(
            rejection.code.as_str(),
            "invalid_request"
                | "not_found"
                | "revision_conflict"
                | "forbidden"
                | "workflow_revoked"
                | "budget_exceeded"
                | "budget_configuration"
                | "context_limit"
        ) {
            return Err(Error::Validation(
                "Transient failures cannot reject an operation".into(),
            ));
        }
        valid_text(&rejection.message, 1000)?;
        self.finish_pending_workflow_operation(project, operation, Some(rejection.clone()))
            .await
    }
    async fn finish_pending_workflow_operation(
        &self,
        project: &str,
        operation: &str,
        rejection: Option<WorkflowOperationRejection>,
    ) -> Result<WorkflowOperation> {
        let before = self.workflow_operation(project, operation).await?;
        let (namespace, target, key) = coordinates(project, &before.request);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let reservation = workflow_write_keys::lock(
            &mut tx,
            namespace,
            target,
            key,
            project,
            &action_identity(project, &before.request),
        )
        .await?;
        if reservation.operation_id.as_deref() != Some(operation) {
            return Err(Error::Conflict);
        }
        let mut current = read_operation(&mut tx, project, operation, true).await?;
        if current.state == WorkflowOperationState::Pending {
            current.state = if rejection.is_some() {
                WorkflowOperationState::Rejected
            } else {
                WorkflowOperationState::Discarded
            };
            current.rejection = rejection;
            current.updated_at = now();
            write_operation(&mut tx, &current).await?;
            sqlx::query("UPDATE workflow_write_keys SET state=$1 WHERE namespace=$2 AND target_id=$3 AND request_key=$4")
                .bind(state_name(&current.state)).bind(namespace).bind(target).bind(key).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(current)
    }
    pub async fn acknowledge_workflow_operation(
        &self,
        project: &str,
        operation: &str,
    ) -> Result<WorkflowOperation> {
        let before = self.workflow_operation(project, operation).await?;
        let (namespace, target, key) = coordinates(project, &before.request);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let reservation = workflow_write_keys::lock(
            &mut tx,
            namespace,
            target,
            key,
            project,
            &action_identity(project, &before.request),
        )
        .await?;
        if reservation.operation_id.as_deref() != Some(operation) {
            return Err(Error::Conflict);
        }
        let mut current = read_operation(&mut tx, project, operation, true).await?;
        if current.state == WorkflowOperationState::Pending {
            return Err(Error::Conflict);
        }
        if current.acknowledged_at.is_none() {
            current.acknowledged_at = Some(now());
            write_operation(&mut tx, &current).await?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(current)
    }
    pub async fn commit_workflow_library_operation(
        &self,
        project: &str,
        operation: &str,
    ) -> Result<WorkflowOperation> {
        let before = self.workflow_operation(project, operation).await?;
        let (namespace, target, key) = coordinates(project, &before.request);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let reservation = workflow_write_keys::lock(
            &mut tx,
            namespace,
            target,
            key,
            project,
            &action_identity(project, &before.request),
        )
        .await?;
        if reservation.operation_id.as_deref() != Some(operation) {
            return Err(Error::Conflict);
        }
        let mut current = read_operation(&mut tx, project, operation, true).await?;
        if current.state != WorkflowOperationState::Pending {
            tx.commit().await.map_err(storage_error)?;
            return Ok(current);
        }
        workflow_write_keys::permit(&reservation, Some(operation))?;
        let receipt = match &current.request {
            WorkflowOperationRequest::Create { request } => {
                let entry = workflows::registry::create_in_tx(
                    &mut tx,
                    project,
                    &request.idempotency_key,
                    &request.definition,
                )
                .await?;
                let first = &entry.versions[0];
                WorkflowOperationReceipt::Created {
                    publication: WorkflowPublication {
                        workflow_id: entry.id,
                        project_id: project.into(),
                        revision: 1,
                        version_id: Some(first.id.clone()),
                        revoked: false,
                        created_at: first.created_at,
                    },
                }
            }
            WorkflowOperationRequest::Publish {
                workflow_id,
                request,
            } => WorkflowOperationReceipt::Published {
                publication: workflows::registry::change_in_tx(
                    &mut tx,
                    project,
                    workflow_id,
                    request.base_revision,
                    &request.idempotency_key,
                    Some(&request.definition),
                )
                .await?,
            },
            WorkflowOperationRequest::Revoke {
                workflow_id,
                request,
            } => WorkflowOperationReceipt::Revoked {
                publication: workflows::registry::change_in_tx(
                    &mut tx,
                    project,
                    workflow_id,
                    request.base_revision,
                    &request.idempotency_key,
                    None,
                )
                .await?,
            },
            WorkflowOperationRequest::Start { .. }
            | WorkflowOperationRequest::ResumeInput { .. } => {
                return Err(Error::Validation(
                    "Start requires trusted runtime admission".into(),
                ));
            }
        };
        accept_operation(&mut tx, &mut current, receipt).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(current)
    }
    pub async fn commit_workflow_run_operation(
        &self,
        project: &str,
        operation: &str,
        spec: RunSpec,
        expected: &WorkflowBinding,
    ) -> Result<WorkflowOperation> {
        let before = self.workflow_operation(project, operation).await?;
        let (namespace, target, key) = coordinates(project, &before.request);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let money_fence = model_money::lock_creation(&mut tx).await?;
        let reservation = workflow_write_keys::lock(
            &mut tx,
            namespace,
            target,
            key,
            project,
            &action_identity(project, &before.request),
        )
        .await?;
        if reservation.operation_id.as_deref() != Some(operation) {
            return Err(Error::Conflict);
        }
        let mut current = read_operation(&mut tx, project, operation, true).await?;
        if current.state != WorkflowOperationState::Pending {
            tx.commit().await.map_err(storage_error)?;
            return Ok(current);
        }
        workflow_write_keys::permit(&reservation, Some(operation))?;
        let WorkflowOperationRequest::Start { request } = &current.request else {
            return Err(Error::Validation(
                "Operation is not a workflow start".into(),
            ));
        };
        if spec.project_id != project {
            return Err(Error::NotFound);
        }
        workflows::product::validate_scope(request, &spec)?;
        if request
            .expected_binding
            .as_ref()
            .is_some_and(|b| canonical_hash(b) != canonical_hash(expected))
        {
            return Err(Error::Conflict);
        }
        let input = super::RunRequest {
            workflow: Some(workflows::product::start(request, Some(expected.clone()))),
            order_id: request.order_id.clone(),
            base_revision: request.base_revision,
            idempotency_key: request.idempotency_key.clone(),
            prompt: request.prompt.clone(),
        };
        let (run, created) = self
            .insert_run_in_tx(
                &mut tx,
                &money_fence,
                &input,
                spec,
                workflows::product::request_hash(request),
                false,
            )
            .await?;
        accept_operation(
            &mut tx,
            &mut current,
            WorkflowOperationReceipt::Started {
                run_id: run.id.clone(),
                spec: Box::new(run.spec.clone()),
                created_at: run.created_at,
            },
        )
        .await?;
        if created {
            self.workflow_clock
                .assert_creation_time(&mut tx, &run)
                .await?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(current)
    }
}
/// The common write key and exact immutable operation precede registry/run locks.
pub(crate) async fn lock_resume(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    operation: &WorkflowOperation,
) -> Result<WorkflowOperation> {
    let (namespace, target, key) = coordinates(&operation.project_id, &operation.request);
    let reservation = workflow_write_keys::lock(
        tx,
        namespace,
        target,
        key,
        &operation.project_id,
        &action_identity(&operation.project_id, &operation.request),
    )
    .await?;
    if reservation.operation_id.as_deref() != Some(&operation.id) {
        return Err(Error::Conflict);
    }
    let current = read_operation(tx, &operation.project_id, &operation.id, true).await?;
    if current.request_sha256 != operation.request_sha256 {
        return Err(Error::Conflict);
    }
    if current.state != WorkflowOperationState::Pending {
        return Ok(current);
    }
    workflow_write_keys::permit(&reservation, Some(&operation.id))?;
    let WorkflowOperationRequest::ResumeInput { request } = &current.request else {
        return Err(Error::Forbidden);
    };
    let payload = sqlx::query_scalar::<_, String>("SELECT payload FROM runs WHERE id=$1")
        .bind(&request.run_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let parent: StoredRun = parse(&payload)?;
    let binding = parent
        .record
        .spec
        .workflow
        .as_ref()
        .ok_or(Error::Forbidden)?;
    workflows::registry::lock_head(tx, &operation.project_id, &binding.workflow_id).await?;
    sqlx::query("UPDATE runs SET revision=revision WHERE id=$1")
        .bind(&request.run_id)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    Ok(current)
}
pub(crate) async fn resume_change(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    store: &Store,
    parent: &mut StoredRun,
    operation: &WorkflowOperation,
) -> Result<Change> {
    let WorkflowOperationRequest::ResumeInput { request } = &operation.request else {
        return Err(Error::Forbidden);
    };
    if parent.record.spec.project_id != operation.project_id
        || parent.record.id != request.run_id
        || parent.record.state != RunState::Interrupted
        || parent.lease.is_some()
    {
        return Err(Error::Conflict);
    }
    let before = workflows::runs::load(tx, &parent.record).await?;
    if parent.record.revision != request.base_run_revision
        || before.progress.revision() != request.base_workflow_revision
        || canonical_hash(&before.progress) != request.expected_checkpoint_sha256
    {
        return Err(Error::Conflict);
    }
    let at = store
        .workflow_clock
        .execution_now(tx, &parent.record)
        .await?;
    let cursors = workflows::continuations::verify_live(tx, parent, &before, at).await?;
    let hash = workflows::continuations::cursor_hash(&cursors);
    if hash != request.expected_cursor_sha256 {
        return Err(Error::Conflict);
    }
    let deadline = cursors
        .iter()
        .map(|c| c.expires_at)
        .min()
        .ok_or(Error::Forbidden)?;
    let mut next = before.clone();
    let generation = next.progress.prepare_human(
        &next.plan,
        before.progress.revision(),
        &operation.id,
        cursors,
        at.div_euclid(1000),
    )?;
    next.format_version = 4;
    parent.transition(RunState::Queued)?;
    parent.record.error_code = None;
    let mut change = Change::event(
        "workflow_input_resume_queued",
        json!({"resume_id":operation.id,"generation":generation,"cursor_sha256":hash}),
    );
    change.workflow_write = Some(workflows::WorkflowWrite {
        new_action: true,
        expected_revision: before.progress.revision(),
        expected_sha256: canonical_hash(&before.progress),
        next,
    });
    change.workflow_resume_operation = Some(Box::new(operation.clone()));
    change.workflow_tool_deadline = Some(deadline);
    Ok(change)
}
pub(crate) async fn write_resume(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &RunRecord,
    change: &Change,
) -> Result<()> {
    let Some(operation) = &change.workflow_resume_operation else {
        return Ok(());
    };
    let workflow = workflows::runs::load(tx, parent).await?;
    let c = workflow
        .progress
        .human_continuations()
        .last()
        .ok_or(Error::Forbidden)?;
    let mut accepted = (**operation).clone();
    let receipt = WorkflowOperationReceipt::InputResumeQueued {
        run_id: parent.id.clone(),
        resume_id: c.id.clone(),
        generation: c.generation,
        accepted_run_revision: parent.revision,
        accepted_workflow_revision: workflow.progress.revision(),
        cursor_sha256: workflows::continuations::cursor_hash(&c.cursors),
        confirmed_sequence: parent.last_sequence,
        accepted_at: parent.updated_at,
    };
    verify_receipt(tx, &accepted, &receipt).await?;
    accept_operation(tx, &mut accepted, receipt).await
}
pub(crate) async fn verify_resume_accepted(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &RunRecord,
    workflow: &StoredWorkflow,
    c: &cowork_domain::workflows::WorkflowHumanContinuation,
) -> Result<()> {
    let operation = read_operation(tx, &parent.spec.project_id, &c.id, false).await?;
    let Some(WorkflowOperationReceipt::InputResumeQueued {
        run_id,
        resume_id,
        generation,
        cursor_sha256,
        ..
    }) = &operation.receipt
    else {
        return Err(Error::Forbidden);
    };
    if operation.state != WorkflowOperationState::Accepted
        || *run_id != parent.id
        || *resume_id != c.id
        || *generation != c.generation
        || *cursor_sha256 != workflows::continuations::cursor_hash(&c.cursors)
        || !workflow.progress.human_continuations().contains(c)
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
pub(crate) async fn read_resume_export(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    runs: &[StoredRun],
) -> Result<Option<Value>> {
    let mut operations = vec![];
    let mut projects = std::collections::BTreeSet::new();
    for run in runs {
        if run.record.spec.workflow.is_some() {
            projects.insert(run.record.spec.project_id.clone());
        }
    }
    for project in projects {
        let ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM workflow_operations WHERE project_id=$1 ORDER BY id",
        )
        .bind(&project)
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
        for id in ids {
            let operation = read_operation(tx, &project, &id, false).await?;
            if let WorkflowOperationRequest::ResumeInput { request } = &operation.request
                && runs.iter().any(|run| {
                    run.record.id == request.run_id && run.record.spec.project_id == project
                })
            {
                operations.push(operation);
            }
        }
    }
    if operations.is_empty() {
        Ok(None)
    } else {
        Ok(Some(
            serde_json::to_value(operations).map_err(storage_error)?,
        ))
    }
}
async fn accept_operation(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    operation: &mut WorkflowOperation,
    receipt: WorkflowOperationReceipt,
) -> Result<()> {
    let (namespace, target, key) = coordinates(&operation.project_id, &operation.request);
    workflow_write_keys::accept(
        tx,
        namespace,
        target,
        key,
        &serde_json::to_value(&receipt).map_err(storage_error)?,
    )
    .await?;
    operation.state = WorkflowOperationState::Accepted;
    operation.receipt = Some(receipt);
    operation.updated_at = match &operation.receipt {
        Some(WorkflowOperationReceipt::InputResumeQueued { accepted_at, .. }) => *accepted_at,
        _ => now(),
    };
    write_operation(tx, operation).await
}
async fn write_operation(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    operation: &WorkflowOperation,
) -> Result<()> {
    sqlx::query("UPDATE workflow_operations SET state=$1,payload=$2,acknowledged_at=$5 WHERE id=$3 AND project_id=$4")
        .bind(state_name(&operation.state)).bind(json_string(operation)?).bind(&operation.id).bind(&operation.project_id).bind(operation.acknowledged_at).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
fn state_name(state: &WorkflowOperationState) -> &'static str {
    match state {
        WorkflowOperationState::Pending => "pending",
        WorkflowOperationState::Accepted => "accepted",
        WorkflowOperationState::Rejected => "rejected",
        WorkflowOperationState::Discarded => "discarded",
    }
}
async fn read_operation(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    id: &str,
    lock: bool,
) -> Result<WorkflowOperation> {
    if lock {
        sqlx::query("UPDATE workflow_operations SET state=state WHERE id=$1 AND project_id=$2")
            .bind(id)
            .bind(project)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    let row=sqlx::query("SELECT request_sha256,state,payload,acknowledged_at,intent_key FROM workflow_operations WHERE id=$1 AND project_id=$2").bind(id).bind(project).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let result: WorkflowOperation =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if result.id != id
        || result.project_id != project
        || result.request_sha256
            != operation_digest(project, &result.request, &result.draft_snapshot)
        || result.request_sha256
            != row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
        || state_name(&result.state) != row.try_get::<String, _>("state").map_err(storage_error)?
        || (result.state == WorkflowOperationState::Accepted) != result.receipt.is_some()
        || (result.state == WorkflowOperationState::Rejected) != result.rejection.is_some()
        || result.acknowledged_at
            != row
                .try_get::<Option<i64>, _>("acknowledged_at")
                .map_err(storage_error)?
        || (result.state == WorkflowOperationState::Pending && result.acknowledged_at.is_some())
    {
        return Err(Error::Forbidden);
    }
    if result.updated_at < result.created_at
        || result.created_at < 0
        || result.intent_id
            != row
                .try_get::<String, _>("intent_key")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    if let Some(reference) = &result.draft_snapshot {
        let snapshot = workflow_drafts::read_snapshot(
            tx,
            project,
            &reference.draft_id,
            &reference.snapshot_id,
        )
        .await?;
        if snapshot.sha256 != reference.sha256 {
            return Err(Error::Forbidden);
        }
    }
    if let Some(receipt) = &result.receipt {
        verify_receipt(tx, &result, receipt).await?;
    }
    Ok(result)
}
async fn verify_receipt(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    operation: &WorkflowOperation,
    receipt: &WorkflowOperationReceipt,
) -> Result<()> {
    let project = &operation.project_id;
    match (&operation.request, receipt) {
        (
            WorkflowOperationRequest::ResumeInput { request },
            WorkflowOperationReceipt::InputResumeQueued {
                run_id,
                resume_id,
                generation,
                accepted_run_revision,
                accepted_workflow_revision,
                cursor_sha256,
                confirmed_sequence,
                accepted_at,
            },
        ) => {
            if request.run_id != *run_id
                || operation.id != *resume_id
                || request.base_run_revision.checked_add(1) != Some(*accepted_run_revision)
                || request.base_workflow_revision.checked_add(1)
                    != Some(*accepted_workflow_revision)
                || request.expected_cursor_sha256 != *cursor_sha256
                || *confirmed_sequence <= 0
            {
                return Err(Error::Forbidden);
            }
            let payload = sqlx::query_scalar::<_, String>("SELECT payload FROM runs WHERE id=$1")
                .bind(run_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::Forbidden)?;
            let parent: StoredRun = parse(&payload)?;
            if parent.record.spec.project_id != *project {
                return Err(Error::Forbidden);
            }
            let workflow = workflows::runs::load(tx, &parent.record).await?;
            let c = workflow
                .progress
                .human_continuations()
                .iter()
                .find(|c| c.id == *resume_id && c.generation == *generation)
                .ok_or(Error::Forbidden)?;
            if workflows::continuations::cursor_hash(&c.cursors) != *cursor_sha256 {
                return Err(Error::Forbidden);
            }
            let payload = sqlx::query_scalar::<_, String>(
                "SELECT payload FROM events WHERE run_id=$1 AND sequence=$2",
            )
            .bind(run_id)
            .bind(confirmed_sequence)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::Forbidden)?;
            let event: RunEvent = parse(&payload)?;
            if event.run_id != *run_id
                || event.sequence != *confirmed_sequence
                || event.created_at != *accepted_at
                || event.kind != "workflow_input_resume_queued"
                || event.schema_version != 8
                || event.payload
                    != json!({"resume_id":resume_id,"generation":generation,"cursor_sha256":cursor_sha256})
            {
                return Err(Error::Forbidden);
            }
        }
        (
            WorkflowOperationRequest::Create { request },
            WorkflowOperationReceipt::Created { publication },
        ) => {
            if publication.project_id != *project
                || publication.revision != 1
                || publication.revoked
            {
                return Err(Error::Forbidden);
            }
            let version = workflows::registry::read_version(
                tx,
                project,
                &publication.workflow_id,
                publication.version_id.as_deref().ok_or(Error::Forbidden)?,
            )
            .await?;
            if version.reference.revision != 1
                || version.reference.created_at != publication.created_at
                || canonical_hash(&version.definition) != canonical_hash(&request.definition)
            {
                return Err(Error::Forbidden);
            }
        }
        (
            WorkflowOperationRequest::Publish {
                workflow_id,
                request,
            },
            WorkflowOperationReceipt::Published { publication },
        ) => {
            if publication.project_id != *project
                || publication.workflow_id != *workflow_id
                || Some(publication.revision) != request.base_revision.checked_add(1)
                || publication.revoked
            {
                return Err(Error::Forbidden);
            }
            let version = workflows::registry::read_version(
                tx,
                project,
                workflow_id,
                publication.version_id.as_deref().ok_or(Error::Forbidden)?,
            )
            .await?;
            if version.reference.revision != publication.revision
                || version.reference.created_at != publication.created_at
                || canonical_hash(&version.definition) != canonical_hash(&request.definition)
            {
                return Err(Error::Forbidden);
            }
        }
        (
            WorkflowOperationRequest::Revoke {
                workflow_id,
                request,
            },
            WorkflowOperationReceipt::Revoked { publication },
        ) => {
            if publication.project_id != *project
                || publication.workflow_id != *workflow_id
                || Some(publication.revision) != request.base_revision.checked_add(1)
                || !publication.revoked
                || publication.version_id.is_some()
            {
                return Err(Error::Forbidden);
            }
            let payload = sqlx::query_scalar::<_, String>(
                "SELECT payload FROM workflow_changes WHERE workflow_id=$1 AND request_key=$2",
            )
            .bind(workflow_id)
            .bind(&request.idempotency_key)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::Forbidden)?;
            let original: WorkflowPublication = parse(&payload)?;
            if canonical_hash(&original) != canonical_hash(publication) {
                return Err(Error::Forbidden);
            }
        }
        (
            WorkflowOperationRequest::Start { request },
            WorkflowOperationReceipt::Started {
                run_id,
                spec,
                created_at,
            },
        ) => {
            let binding = spec.workflow.as_ref().ok_or(Error::Forbidden)?;
            if spec.project_id != *project
                || spec.order_id != request.order_id
                || spec.prompt != request.prompt
                || spec.access_mode != request.access_mode
                || binding.workflow_id != request.workflow_id
                || binding.version_id != request.version_id
                || request
                    .expected_binding
                    .as_ref()
                    .is_some_and(|b| b != binding)
            {
                return Err(Error::Forbidden);
            }
            let row =
                sqlx::query("SELECT payload,request_hash FROM runs WHERE id=$1 AND order_id=$2")
                    .bind(run_id)
                    .bind(&request.order_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(storage_error)?
                    .ok_or(Error::Forbidden)?;
            let stored: StoredRun =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let frozen = workflows::runs::load(tx, &stored.record).await?;
            if canonical_hash(&frozen.plan.scope.budgets) != canonical_hash(&request.budgets)
                || canonical_hash(&frozen.plan.budgets) != canonical_hash(&spec.budgets)
            {
                return Err(Error::Forbidden);
            }
            if stored.record.id != *run_id
                || canonical_hash(&stored.record.spec) != canonical_hash(spec)
                || stored.record.created_at != *created_at
                || row
                    .try_get::<String, _>("request_hash")
                    .map_err(storage_error)?
                    != workflows::product::request_hash(request)
            {
                return Err(Error::Forbidden);
            }
        }
        _ => return Err(Error::Forbidden),
    }
    Ok(())
}
async fn check_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    request: &WorkflowOperationRequest,
) -> Result<()> {
    match request {
        WorkflowOperationRequest::ResumeInput { request } => {
            let payload = sqlx::query_scalar::<_, String>("SELECT payload FROM runs WHERE id=$1")
                .bind(&request.run_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
            let run: StoredRun = parse(&payload)?;
            if run.record.spec.project_id != project || run.record.spec.workflow.is_none() {
                return Err(Error::NotFound);
            }
            Ok(())
        }
        WorkflowOperationRequest::Create { .. } => Ok(()),
        WorkflowOperationRequest::Publish { workflow_id, .. }
        | WorkflowOperationRequest::Revoke { workflow_id, .. } => {
            workflows::registry::read_entry(tx, project, workflow_id).await?;
            Ok(())
        }
        WorkflowOperationRequest::Start { request } => {
            let order =
                sqlx::query_scalar::<_, String>("SELECT project_id FROM orders WHERE id=$1")
                    .bind(&request.order_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(storage_error)?
                    .ok_or(Error::NotFound)?;
            if order != project {
                return Err(Error::NotFound);
            }
            workflows::registry::read_version(
                tx,
                project,
                &request.workflow_id,
                &request.version_id,
            )
            .await?;
            Ok(())
        }
    }
}
async fn legacy_effect(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    namespace: &str,
    target: &str,
    key: &str,
) -> Result<bool> {
    let query = match namespace {
        "workflow-input-resume" => return Ok(false),
        "workflow-create" => "SELECT id FROM workflows WHERE project_id=$1 AND request_key=$2",
        "workflow-change" => {
            "SELECT workflow_id FROM workflow_changes WHERE workflow_id=$1 AND request_key=$2"
        }
        "run" => "SELECT id FROM runs WHERE order_id=$1 AND idempotency_key=$2",
        _ => return Err(Error::Forbidden),
    };
    Ok(sqlx::query_scalar::<_, String>(query)
        .bind(target)
        .bind(key)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .is_some())
}
