//! Projection and frozen receipt checks inside the event transaction. No IO.
use super::*;

fn unavailable() -> Error {
    Error::Validation("automation_inputs_workflow_unavailable".into())
}
fn version_error(error: Error) -> Error {
    match error {
        Error::NotFound | Error::Forbidden | Error::Validation(_) => unavailable(),
        other => other,
    }
}
async fn version(
    tx: &mut Transaction<'_, Any>,
    project: &str,
    task: &WorkspaceTask,
) -> Result<Box<crate::workflows::WorkflowVersion>> {
    let WorkspaceTaskExecution::Workflow {
        workflow_id,
        version_id,
        ..
    } = &task.value.execution
    else {
        return Err(Error::Validation(
            "automation_inputs_workflow_required".into(),
        ));
    };
    let entry = crate::workflows::registry::read_entry(tx, project, workflow_id)
        .await
        .map_err(version_error)?;
    if entry.revoked {
        return Err(unavailable());
    }
    Ok(Box::new(
        crate::workflows::registry::read_version(tx, project, workflow_id, version_id)
            .await
            .map_err(version_error)?,
    ))
}
pub(super) async fn validate(
    tx: &mut Transaction<'_, Any>,
    project: &str,
    task: &WorkspaceTask,
    mapping: Option<&AutomationWorkflowInputs>,
) -> Result<()> {
    if let Some(mapping) = mapping {
        let version = version(tx, project, task).await?;
        cowork_domain::automation::validate_input_bindings(mapping, &version.definition)?;
    }
    Ok(())
}
pub(super) async fn frozen_task(
    tx: &mut Transaction<'_, Any>,
    trigger: &AutomationTrigger,
) -> Result<WorkspaceTask> {
    let raw: String = sqlx::query_scalar(
        "SELECT task_snapshot FROM automation_triggers WHERE id=$1 AND project_id=$2",
    )
    .bind(&trigger.id)
    .bind(&trigger.project_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    let task: WorkspaceTask = parse(&raw)?;
    if task.id != trigger.task_id || task.revision != trigger.task_revision {
        return Err(Error::Forbidden);
    }
    Ok(task)
}
pub(super) async fn snapshot(
    tx: &mut Transaction<'_, Any>,
    trigger: &AutomationTrigger,
    task: &WorkspaceTask,
    data: &Value,
) -> Result<Option<Box<AutomationWorkflowInputSnapshot>>> {
    let Some(mapping) = trigger.workflow_inputs.as_deref() else {
        return Ok(None);
    };
    let version = version(tx, &trigger.project_id, task).await?;
    let WorkspaceTaskExecution::Workflow { inputs, .. } = &task.value.execution else {
        return Err(unavailable());
    };
    let values = cowork_domain::automation::resolve_workflow_inputs(
        mapping,
        &version.definition,
        inputs.as_deref(),
        data,
    )?;
    Ok(Some(Box::new(AutomationWorkflowInputSnapshot {
        workflow_id: version.workflow_id.clone(),
        version_id: version.reference.id.clone(),
        definition_sha256: version.reference.sha256.clone(),
        mapping_sha256: canonical_hash(mapping),
        values: Box::new(values),
    })))
}
pub(super) fn error_code(error: &Error) -> Option<&'static str> {
    match error {
        Error::Validation(reason) if reason.starts_with("automation_inputs_") => Some(error.code()),
        _ => None,
    }
}
pub(super) fn verify_receipt(event: &AutomationEvent, raw: &str) -> Result<()> {
    let original: AutomationEvent = parse(raw)?;
    let identity = |event: &AutomationEvent| {
        canonical_hash(&json!({
            "id":event.id,"project":event.project_id,"trigger":event.trigger_id,"trigger_revision":event.trigger_revision,
            "task":event.task_id,"task_revision":event.task_revision,"event_id":event.event_id,"source":event.source,
            "data":event.data,"workflow_input":event.workflow_input,"received_at":event.received_at,
        }))
    };
    if identity(event) != identity(&original) {
        return Err(Error::Forbidden);
    }
    Ok(())
}
