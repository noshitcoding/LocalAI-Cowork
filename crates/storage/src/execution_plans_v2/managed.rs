//! Shared database authority checks for the explicit managed destination.
use super::*;
use cowork_domain::environments::{
    EnvironmentArtifact, EnvironmentNetwork, EnvironmentVersionState,
};
use cowork_workflow_contracts::OwnerExecutionTargetV2;

impl StoredExecutionPlanV2 {
    pub(super) fn destination_scope(&self) -> bool {
        let p = &self.plan;
        match &p.target {
            ExecutionDestinationV2::BrokerController { .. } => {
                self.request
                    .owner()
                    .is_none_or(|r| matches!(r.target, OwnerExecutionTargetV2::BrokerController {}))
                    && broker_tools(&self.source_spec)
                    && broker_tools(&self.spec)
                    && p.boundary.environment.is_none()
                    && p.boundary.process_access == ExecutionProcessAccess::BrokerOnly
                    && p.boundary.process_network == ExecutionNetworkAccess::NoProcess
            }
            ExecutionDestinationV2::DocumentEngine {
                engine_revision, ..
            } => super::documents::destination_scope(self, engine_revision),
            ExecutionDestinationV2::ManagedRunner {
                target,
                runner,
                programs,
                ..
            } => {
                self.request.owner().is_some_and(|r| {
                    matches!(r.target, OwnerExecutionTargetV2::RegisteredRunner {})
                        && matches!(r.request, PublicRequest::Single(_))
                }) && cowork_domain::run_workspaces::managed_process_scope(&self.source_spec)
                    && cowork_domain::run_workspaces::managed_process_scope(&self.spec)
                    && p.boundary.environment == self.spec.environment
                    && p.boundary.process_access == ExecutionProcessAccess::QualifiedEnvironment
                    && p.boundary.process_network == ExecutionNetworkAccess::Denied
                    && self.spec.folders.iter().all(|f| f.writable)
                    && runner.runner_id == self.spec.executor
                    && cowork_domain::execution_targets::validate_runner_identity(runner).is_ok()
                    && target.revision > 0
                    && cowork_domain::execution_targets::execution_target_id(runner)
                        .is_ok_and(|id| id == target.target_id)
                    && cowork_domain::execution_targets::runner_identity_hash(runner)
                        .is_ok_and(|sha| sha == target.identity_sha256)
                    && !programs.is_empty()
                    && programs.len() <= 128
            }
        }
    }
}

pub(crate) async fn require_current(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    saved: &StoredExecutionPlanV2,
) -> Result<()> {
    let ExecutionDestinationV2::ManagedRunner {
        target,
        runner,
        programs,
        ..
    } = &saved.plan.target
    else {
        return Ok(());
    };
    let registered =
        crate::execution_targets::check_reference_in_transaction(tx, target, true).await?;
    if registered.identity != *runner {
        return Err(Error::ExecutionPlanChanged);
    }
    let reference = saved
        .spec
        .environment
        .as_ref()
        .ok_or(Error::ExecutionPlanChanged)?;
    sqlx::query("UPDATE environments SET revision=revision WHERE project_id=$1 AND id=$2")
        .bind(&saved.plan.project_id)
        .bind(&reference.environment_id)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    let row =
        sqlx::query("SELECT revision,payload FROM environments WHERE project_id=$1 AND id=$2")
            .bind(&saved.plan.project_id)
            .bind(&reference.environment_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::ExecutionPlanChanged)?;
    let environment =
        crate::environments::checked(&row, &saved.plan.project_id, &reference.environment_id)?;
    let version = environment.version(&reference.version_id)?;
    let EnvironmentVersionState::Qualified { qualification } = &version.state else {
        return Err(Error::ExecutionPlanChanged);
    };
    if environment.revoked
        || version.revoked
        || environment.active_version_id.as_deref() != Some(reference.version_id.as_str())
        || version.sha256 != reference.definition_sha256
        || canonical_hash(qualification) != reference.qualification_sha256
        || qualification.runner_id != runner.runner_id
        || qualification.runner_generation != runner.generation
        || qualification.adapter_build_sha256 != runner.adapter_build_sha256
        || !matches!(
            version.definition.artifact,
            EnvironmentArtifact::ManagedLinuxPackage { .. }
        )
        || version.definition.network != Some(EnvironmentNetwork::Denied)
        || &version.definition.programs != programs
    {
        return Err(Error::ExecutionPlanChanged);
    }
    Ok(())
}
