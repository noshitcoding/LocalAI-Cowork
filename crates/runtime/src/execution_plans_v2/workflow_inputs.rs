//! Original workflow data is read natively; no old scope or plan is a grant.
use super::*;
use cowork_workflow_contracts::{WorkflowInputSnapshot, WorkflowInputSource};

impl Runtime {
    pub async fn workflow_input_snapshot(&self, run: &str) -> Result<WorkflowInputSnapshot> {
        valid_text(run, 128)?;
        let record = self.store.stored_run(run).await?.record;
        let order = self.store.order(&record.spec.order_id).await?;
        if record.id != run || order.project_id != record.spec.project_id {
            return Err(Error::Forbidden);
        }
        self.store
            .native_chat_available(&record.spec.order_id)
            .await?;
        let workflow = record.spec.workflow.ok_or(Error::Forbidden)?;
        let inputs = workflow
            .inputs
            .ok_or_else(|| Error::Validation("workflow_input_source_empty".into()))?;
        let canonical_json = serde_json::to_string(&inputs).map_err(storage_error)?;
        Ok(WorkflowInputSnapshot {
            source: WorkflowInputSource {
                run_id: record.id,
                sha256: hash(canonical_json.as_bytes()),
            },
            project_id: record.spec.project_id,
            workflow_id: workflow.workflow_id,
            version_id: workflow.version_id,
            definition_sha256: workflow.definition_sha256,
            canonical_json,
        })
    }

    pub(super) async fn resolve_workflow_input_source(
        &self,
        input: &RunRequest,
    ) -> Result<RunRequest> {
        let RunRequest::Workflow(request) = input else {
            return Ok(input.clone());
        };
        let Some(source) = &request.input_source else {
            return Ok(input.clone());
        };
        if request.inputs.is_some()
            || request.expected_binding.is_some()
            || !cowork_domain::workspace_transfer::valid_sha256(&source.sha256)
        {
            return Err(Error::Validation("workflow_input_source_mismatch".into()));
        }
        let order = self.store.order(&request.order_id).await?;
        let original = self.workflow_input_snapshot(&source.run_id).await?;
        if original.project_id != order.project_id
            || original.source != *source
            || original.workflow_id != request.workflow_id
            || original.version_id != request.version_id
        {
            return Err(Error::Validation("workflow_input_source_mismatch".into()));
        }
        let version = self
            .store
            .workflow_version(&order.project_id, &request.workflow_id, &request.version_id)
            .await?;
        if version.reference.sha256 != original.definition_sha256 {
            return Err(Error::ExecutionPlanChanged);
        }
        let mut resolved = request.clone();
        resolved.inputs = Some(Box::new(
            serde_json::from_str(&original.canonical_json).map_err(storage_error)?,
        ));
        resolved.input_source = None;
        Ok(RunRequest::Workflow(resolved))
    }
}
