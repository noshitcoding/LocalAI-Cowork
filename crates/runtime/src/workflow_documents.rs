//! Document results use the original workflow attempt and source evidence.
//! A recovered publication is observed, never generated a second time.
use super::*;
use cowork_storage::{
    WorkflowDocumentContext, WorkflowToolExecution, workflow_toolbox::NativeWorkflowAdmission,
};

pub(super) fn document_tool(name: &str) -> bool {
    matches!(name, "document_read" | "document_write" | "image_edit")
}

pub(super) fn definition_uses_ocr(
    definition: &cowork_domain::workflows::WorkflowDefinition,
) -> bool {
    definition.documents.values().any(|step| {
        matches!(
            step.operation,
            cowork_domain::workflows::WorkflowDocumentOperation::Ocr { .. }
        )
    }) || definition
        .subflows
        .values()
        .any(|call| definition_uses_ocr(&call.definition))
        || definition
            .iterations
            .values()
            .any(|call| definition_uses_ocr(&call.definition))
}

pub(super) fn definition_uses_documents(
    definition: &cowork_domain::workflows::WorkflowDefinition,
) -> bool {
    let uses = |rights: &cowork_domain::workflows::WorkflowRights| {
        rights.tools.iter().any(|name| document_tool(name))
    };
    definition.uses_document_steps()
        || uses(&definition.rights)
        || definition.roles.iter().any(|role| uses(&role.rights))
        || definition.steps.iter().any(|step| uses(&step.rights))
        || definition
            .subflows
            .values()
            .any(|call| uses(&call.rights) || definition_uses_documents(&call.definition))
        || definition
            .iterations
            .values()
            .any(|call| uses(&call.rights) || definition_uses_documents(&call.definition))
}

impl Runtime {
    pub(super) async fn invoke_workflow_document(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn: &str,
        raw: &str,
        stop: &CancellationToken,
    ) -> Result<WorkflowToolExecution> {
        let context = self
            .store
            .workflow_document_context(project, run, lease, turn, raw)
            .await?;
        if let Some(receipt) = &context.completed {
            return Ok(WorkflowToolExecution::Completed(receipt.clone()));
        }
        self.assert_snapshot_controller(&context.record.spec)
            .await?;
        if context.engine_revision.as_ref().is_some_and(|expected| {
            self.toolbox
                .as_ref()
                .is_none_or(|toolbox| &toolbox.document_engine_revision() != expected)
        }) {
            return Err(Error::ExecutionPlanChanged);
        }
        let remaining = context
            .deadline_ms
            .checked_sub(now())
            .filter(|left| *left > 0)
            .ok_or(Error::BudgetExceeded)? as u64;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(remaining);
        let precheck = tokio::select! {
            biased;
            _=stop.cancelled()=>return Err(Error::Provider("shutdown".into())),
            _=tokio::time::sleep_until(deadline)=>return Err(Error::BudgetExceeded),
            result=self.document_precheck(&context.record, &context.tool)=>result,
        };
        if let Err(error) = precheck {
            if matches!(error, Error::Forbidden) {
                return Err(error);
            }
            let receipt = self
                .store
                .reject_workflow_document_precheck(project, run, lease, turn, raw, &error)
                .await?;
            return Ok(WorkflowToolExecution::Completed(receipt));
        }
        let (record, tool) = match self
            .store
            .admit_workflow_toolbox(project, run, lease, turn, raw)
            .await?
        {
            NativeWorkflowAdmission::Dispatch { record, tool } => (*record, tool),
            NativeWorkflowAdmission::Completed(receipt) => {
                return Ok(WorkflowToolExecution::Completed(receipt));
            }
            NativeWorkflowAdmission::AwaitingApproval(approval) => {
                return Ok(WorkflowToolExecution::AwaitingApproval(approval));
            }
        };
        if canonical_hash(&record.spec) != canonical_hash(&context.record.spec)
            || tool != context.tool
        {
            return Err(Error::Conflict);
        }
        self.store
            .assert_workflow_toolbox_dispatch(project, run, lease, turn, raw)
            .await?;
        let execution = match Box::pin(self.prepare_document_execution(
            &record,
            lease,
            &tool,
            stop,
            deadline,
            context.engine_revision.as_deref(),
        ))
        .await?
        {
            Ok(execution) => execution,
            Err(error) => {
                let value = json!({"error":error.code(),"message":error.to_string(),"_workflow_success":false});
                self.store
                    .retain_workflow_external_preparation(
                        project,
                        run,
                        turn,
                        raw,
                        "file_read",
                        json!({"document":value,"confirmed_engine_failure":true}),
                    )
                    .await?;
                return self
                    .finish_workflow_document_result(&context, lease, turn, raw, value)
                    .await;
            }
        };
        let metadata = json!({"engine":execution.result,"cowork_images":execution.images,
            "document_engine_revision":execution.engine_revision});
        let Some(publication) = execution.publication else {
            let mut value = metadata;
            value["_workflow_success"] = json!(execution.succeeded);
            self.store
                .retain_workflow_external_preparation(
                    project,
                    run,
                    turn,
                    raw,
                    "file_read",
                    json!({"document":value}),
                )
                .await?;
            return self
                .finish_workflow_document_result(&context, lease, turn, raw, value)
                .await;
        };
        self.store
            .assert_workflow_toolbox_dispatch(project, run, lease, turn, raw)
            .await?;
        self.assert_snapshot_controller(&record.spec).await?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let publishing = tool.clone();
        let task = tokio::task::spawn_blocking(move || {
            files.prepare_generated(
                &record,
                &publishing,
                &publication.folder_id,
                &publication.path,
                &publication.content,
                publication.provenance,
            )
        });
        let prepared = match self
            .wait_file_task(run, lease, stop, deadline, task)
            .await?
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let value = json!({"error":error.code(),"message":error.to_string(),"_workflow_success":false});
                self.store.retain_workflow_external_preparation(project, run, turn, raw, "file_read",
                    json!({"document":value,"publication_not_started":true,"engine_attempt":metadata})).await?;
                return self
                    .finish_workflow_document_result(&context, lease, turn, raw, value)
                    .await;
            }
        };
        let operation = prepared.operation.clone();
        self.store
            .retain_workflow_external_preparation(
                project,
                run,
                turn,
                raw,
                "file_operation",
                json!({"operation_receipt":operation,"document":metadata}),
            )
            .await?;
        self.store
            .assert_workflow_toolbox_dispatch(project, run, lease, turn, raw)
            .await?;
        if stop.is_cancelled() {
            return Err(Error::UncertainEffect);
        }
        let task = tokio::task::spawn_blocking(move || (prepared.apply(), prepared));
        let (applied, guard) = self
            .wait_file_task(run, lease, stop, deadline, task)
            .await?;
        let value = match applied {
            Ok(()) => {
                let mut value = metadata;
                value["operation_receipt"] = json!(operation);
                value["_workflow_success"] = json!(true);
                value
            }
            Err(Error::FileConflict) => json!({"error":"file_conflict","_workflow_success":false}),
            Err(_) => return Err(Error::UncertainEffect),
        };
        let completed = self
            .finish_workflow_document_result(&context, lease, turn, raw, value)
            .await;
        drop(guard);
        completed
    }

    async fn finish_workflow_document_result(
        &self,
        context: &WorkflowDocumentContext,
        lease: &str,
        turn: &str,
        raw: &str,
        value: Value,
    ) -> Result<WorkflowToolExecution> {
        let (project, run, call) = (
            &context.record.spec.project_id,
            &context.record.id,
            &context.tool.id,
        );
        self.store
            .retain_workflow_external_outcome(project, run, call, value.clone())
            .await?;
        // Retry only the confirmed receipt transaction after sibling CAS changes.
        for _ in 0..64 {
            match self
                .store
                .finish_workflow_toolbox(project, run, lease, turn, raw, value.clone())
                .await
            {
                Ok(receipt) => return Ok(WorkflowToolExecution::Completed(receipt)),
                Err(Error::Conflict) => tokio::task::yield_now().await,
                Err(error) => return Err(error),
            }
        }
        Err(Error::Conflict)
    }
}
