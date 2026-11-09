//! Explicit host document engine destination. It never widens BrokerOnly.
use super::*;

impl Runtime {
    pub(crate) async fn reject_broker_document_workflow(&self, request: &RunRequest) -> Result<()> {
        let RunRequest::Workflow(request) = request else {
            return Ok(());
        };
        let order = self.store.order(&request.order_id).await?;
        let version = self
            .store
            .workflow_version(&order.project_id, &request.workflow_id, &request.version_id)
            .await?;
        if crate::workflow_documents::definition_uses_documents(&version.definition) {
            return Err(Error::Validation(
                "workflow_document_target_required".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn document_execution_destination(
        &self,
        spec: &RunSpec,
        controller: BrokerControllerRef,
    ) -> Result<ExecutionDestinationV2> {
        if !cowork_storage::document_execution_scope(spec) {
            return Err(Error::Validation(
                "workflow_document_runtime_unavailable".into(),
            ));
        }
        self.require_document_tools()?;
        let engine_revision = self
            .toolbox
            .as_ref()
            .ok_or(Error::Forbidden)?
            .document_engine_revision();
        Ok(ExecutionDestinationV2::DocumentEngine {
            controller,
            engine_revision,
        })
    }
}
