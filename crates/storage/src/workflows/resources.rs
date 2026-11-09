//! Resolve source selections from the already admitted run scope, never an ID
//! supplied by the workflow definition or inferred from a filesystem path.
use super::*;

pub(super) fn workspace_folder(
    definition: &WorkflowDefinition,
    spec: &RunSpec,
) -> Result<Option<WorkflowWorkspaceBinding>> {
    if !definition.uses_current_workspace() {
        return Ok(None);
    }
    let mut managed = spec
        .folders
        .iter()
        .filter(|folder| folder.order_id.as_deref() == Some(spec.order_id.as_str()));
    let folder = managed
        .next()
        .ok_or_else(|| Error::Validation("workflow_current_workspace_unavailable".into()))?;
    if managed.next().is_some() {
        return Err(Error::Validation(
            "workflow_current_workspace_ambiguous".into(),
        ));
    }
    if folder.project_id != spec.project_id {
        return Err(Error::Forbidden);
    }
    WorkflowWorkspaceBinding::from_grant(folder).map(Some)
}
