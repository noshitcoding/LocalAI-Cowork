//! Durable commands reuse the existing compiler and SQL acceptance boundary.
use super::*;
use cowork_workflow_contracts::{
    WorkflowOperation, WorkflowOperationRejection, WorkflowOperationRequest,
    WorkflowOperationState, WorkflowRunRequest,
};

impl Runtime {
    /// Only the persisted operation identifies the original command. Neither a
    /// new request body nor the current editor can replace its frozen intent.
    pub async fn commit_workflow_operation(
        &self,
        project: &str,
        operation_id: &str,
    ) -> Result<WorkflowOperation> {
        let operation = self.store.workflow_operation(project, operation_id).await?;
        if operation.state != WorkflowOperationState::Pending {
            return Ok(operation);
        }
        let result = match &operation.request {
            WorkflowOperationRequest::ResumeInput { .. } => {
                self.store
                    .commit_workflow_input_resume_operation(project, operation_id)
                    .await
            }
            WorkflowOperationRequest::Start { request } => {
                self.commit_journal_start(project, operation_id, request)
                    .await
            }
            _ => {
                self.store
                    .commit_workflow_library_operation(project, operation_id)
                    .await
            }
        };
        match result {
            Ok(operation) => Ok(operation),
            Err(error) => {
                self.reconcile_journal_commit(project, operation_id, error)
                    .await
            }
        }
    }

    async fn commit_journal_start(
        &self,
        project: &str,
        operation_id: &str,
        input: &WorkflowRunRequest,
    ) -> Result<WorkflowOperation> {
        // This awaits live providers/credentials and conservative context checks
        // without owning any journal, workflow, order or provider SQL lock.
        let flight = self.preflight_workflow(input).await?;
        let order = self.store.order(&input.order_id).await?;
        if order.project_id != project {
            return Err(Error::NotFound);
        }
        // The only production mutation of this vector commits its provider INSERT
        // before requesting the write guard. Boot builds the vector before Runtime.
        // Hold this read guard through SQL acceptance, never through provider IO.
        let configured = self.providers.read().await;
        for role in &flight.roles {
            let frozen = &role.provider;
            let provider = configured
                .iter()
                .find(|p| p.profile.id == frozen.id)
                .ok_or_else(|| Error::Provider("profile_missing".into()))?;
            if canonical_hash(&provider.profile) != canonical_hash(frozen) {
                return Err(Error::Provider("profile_changed".into()));
            }
            if frozen.credential_required
                && self
                    .personal_provider_credential_from(provider)
                    .await?
                    .is_none()
            {
                return Err(Error::Provider("credential_missing".into()));
            }
        }
        // Storage locks the common write key, rechecks the actual operation and
        // every frozen binding/policy/profile, then writes effect+receipt together.
        self.store
            .commit_workflow_run_operation(
                project,
                operation_id,
                self.resolve_workflow_scope(input, &order).await?,
                &flight.workflow,
            )
            .await
    }

    async fn reconcile_journal_commit(
        &self,
        project: &str,
        operation_id: &str,
        error: Error,
    ) -> Result<WorkflowOperation> {
        // A concurrent accepted/discarded command wins even if this attempt's
        // live preflight or final SQL operation returned an error afterwards.
        let current = self.store.workflow_operation(project, operation_id).await?;
        if current.state != WorkflowOperationState::Pending {
            return Ok(current);
        }
        if !matches!(
            error,
            Error::Validation(_)
                | Error::NotFound
                | Error::Conflict
                | Error::Forbidden
                | Error::WorkflowRevoked
                | Error::BudgetExceeded
                | Error::BudgetConfiguration
                | Error::ContextLimit
        ) {
            // Provider/storage/uncertain and resource-busy failures do not prove
            // a durable rejection. Keep the same Pending intent for reconciliation.
            return Err(error);
        }
        let rejection = WorkflowOperationRejection {
            code: error.code().into(),
            message: error.to_string(),
        };
        // This final transition takes the same key fence and returns any terminal
        // state that won meanwhile; it never overwrites Accepted or Discarded.
        self.store
            .reject_pending_workflow_operation(project, operation_id, &rejection)
            .await
    }
}
