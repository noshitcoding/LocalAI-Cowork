//! The document target is authenticated by the original stored plan and current
//! controller. Its engine revision is data to compare before launch, not a grant.
use super::*;
use cowork_workflow_contracts::OwnerExecutionTargetV2;

pub(super) fn destination_scope(saved: &StoredExecutionPlanV2, revision: &str) -> bool {
    saved
        .request
        .owner()
        .is_some_and(|request| matches!(request.target, OwnerExecutionTargetV2::DocumentEngine {}))
        && cowork_domain::workspace_transfer::valid_sha256(revision)
        && crate::document_execution_scope(&saved.source_spec)
        && crate::document_execution_scope(&saved.spec)
        && saved.plan.boundary.environment.is_none()
        && saved.plan.boundary.process_access == ExecutionProcessAccess::UserRights
        && saved.plan.boundary.process_network == ExecutionNetworkAccess::UserRights
}

impl Store {
    pub async fn pinned_document_engine_revision(&self, spec: &RunSpec) -> Result<Option<String>> {
        let Some(binding) = &spec.input_workspace else {
            // A saved task or internal native workflow cannot infer process consent.
            return Err(Error::Forbidden);
        };
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::execution_controllers::require_current(&mut tx, spec, None).await?;
        let row = sqlx::query("SELECT p.* FROM owner_run_workspaces w JOIN owner_execution_plans_v2 p ON p.id=w.plan_id WHERE w.workspace_id=$1 AND p.run_id=w.run_id")
            .bind(&binding.id).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::ExecutionPlanChanged)?;
        let saved = checked(&row)?;
        let ExecutionDestinationV2::DocumentEngine {
            engine_revision, ..
        } = saved.plan.target
        else {
            return Err(Error::Forbidden);
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(engine_revision))
    }
}
