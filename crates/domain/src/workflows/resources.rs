//! Explicit source selections become concrete rights before normal compilation.
//! A selector is never a folder grant, a native identity or dispatch authority.
use super::*;

/// Logical task selection. The enclosing RunSpec/ExecutionPlan binds the actual
/// resource, revision and copy; this compiler value never authorizes native IO.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowWorkspaceBinding {
    pub folder_id: String,
    pub project_id: String,
    pub order_id: String,
}
impl WorkflowWorkspaceBinding {
    pub fn from_grant(folder: &cowork_contracts::FolderGrant) -> Result<Self> {
        let order_id = folder.order_id.as_ref().ok_or(Error::Forbidden)?;
        for value in [&folder.id, &folder.project_id, order_id] {
            valid_text(value, 128)?;
        }
        Ok(Self {
            folder_id: folder.id.clone(),
            project_id: folder.project_id.clone(),
            order_id: order_id.clone(),
        })
    }
}

impl WorkflowDefinition {
    pub fn uses_current_workspace(&self) -> bool {
        self.documents.values().any(|step| {
            step.rights.current_workspace
                || matches!(step.folder, WorkflowDocumentFolder::CurrentWorkspace)
        }) || self.rights.current_workspace
            || self.roles.iter().any(|role| role.rights.current_workspace)
            || self.steps.iter().any(|step| step.rights.current_workspace)
            || self.subflows.values().any(|call| {
                call.rights.current_workspace || call.definition.uses_current_workspace()
            })
            || self.iterations.values().any(|call| {
                call.rights.current_workspace || call.definition.uses_current_workspace()
            })
    }

    pub(super) fn resolve_current_workspace(&self, scope: &WorkflowScope) -> Result<Self> {
        let folder = scope
            .workspace_folder
            .as_ref()
            .ok_or_else(|| Error::Validation("workflow_current_workspace_unavailable".into()))?;
        if !scope.rights.folders.contains(&folder.folder_id) {
            return Err(Error::Forbidden);
        }
        for value in [&folder.folder_id, &folder.project_id, &folder.order_id] {
            valid_text(value, 128)?;
        }
        fn rights(value: &mut WorkflowRights, id: &str) {
            if value.current_workspace {
                value.folders.insert(id.into());
                value.current_workspace = false;
            }
        }
        fn bind(value: &mut WorkflowDefinition, id: &str) {
            rights(&mut value.rights, id);
            for step in value.documents.values_mut() {
                rights(&mut step.rights, id);
                if matches!(step.folder, WorkflowDocumentFolder::CurrentWorkspace) {
                    step.folder = WorkflowDocumentFolder::SelectedFolder {
                        folder_id: id.into(),
                    };
                }
            }
            for role in &mut value.roles {
                rights(&mut role.rights, id);
            }
            for step in &mut value.steps {
                rights(&mut step.rights, id);
            }
            for call in value.subflows.values_mut() {
                rights(&mut call.rights, id);
                bind(&mut call.definition, id);
            }
            for call in value.iterations.values_mut() {
                rights(&mut call.rights, id);
                bind(&mut call.definition, id);
            }
        }
        let mut resolved = self.clone();
        bind(&mut resolved, &folder.folder_id);
        Ok(resolved)
    }
}
