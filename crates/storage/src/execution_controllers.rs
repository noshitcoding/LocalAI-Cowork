use super::*;

pub(super) async fn require_current(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    spec: &RunSpec,
    origin: Option<&str>,
) -> Result<()> {
    let Some(binding) = &spec.input_workspace else {
        return Ok(());
    };
    let row = sqlx::query("SELECT p.* FROM owner_run_workspaces w JOIN owner_execution_plans_v2 p ON p.id=w.plan_id WHERE w.workspace_id=$1 AND p.run_id=w.run_id")
        .bind(&binding.id).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::ExecutionPlanChanged)?;
    let saved = execution_plans_v2::checked(&row)?;
    if &saved.workspace.binding != binding
        || saved.plan.order_id != spec.order_id
        || saved.plan.project_id != spec.project_id
        || match &saved.plan.target {
            cowork_workflow_contracts::ExecutionDestinationV2::BrokerController { .. } => {
                !cowork_domain::run_workspaces::broker_scope(spec)
            }
            cowork_workflow_contracts::ExecutionDestinationV2::DocumentEngine { .. } => {
                !crate::document_execution_scope(spec)
            }
            cowork_workflow_contracts::ExecutionDestinationV2::ManagedRunner { .. } => {
                spec.environment != saved.spec.environment
                    || spec.executor != saved.spec.executor
                    || !spec.mcp_tools.is_empty()
            }
        }
        || canonical_hash(&spec.file_action) != canonical_hash(&saved.spec.file_action)
    {
        return Err(Error::ExecutionPlanChanged);
    }
    for folder in &spec.folders {
        let original = saved
            .spec
            .folders
            .iter()
            .find(|f| f.id == folder.id)
            .ok_or(Error::Forbidden)?;
        let mut narrowed = original.clone();
        narrowed.writable = folder.writable;
        if narrowed != *folder || folder.writable && !original.writable {
            return Err(Error::Forbidden);
        }
    }
    if spec
        .allowed_tools
        .iter()
        .any(|t| !saved.spec.allowed_tools.contains(t))
    {
        return Err(Error::Forbidden);
    }
    let witness: String =
        sqlx::query_scalar("SELECT origin_sha256 FROM broker_controller WHERE singleton=1")
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::ExecutionPlanChanged)?;
    if origin.is_some_and(|v| v != witness) {
        return Err(Error::ExecutionPlanChanged);
    }
    if locked(tx, &witness).await? != *saved.plan.target.controller() {
        return Err(Error::ExecutionPlanChanged);
    }
    execution_plans_v2::managed::require_current(tx, &saved).await?;
    Ok(())
}

pub(super) async fn locked(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    origin: &str,
) -> Result<BrokerControllerRef> {
    if !cowork_domain::workspace_transfer::valid_sha256(origin) {
        return Err(Error::Forbidden);
    }
    sqlx::query("UPDATE broker_controller SET generation=generation WHERE singleton=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    let row =
        sqlx::query("SELECT id,generation,origin_sha256 FROM broker_controller WHERE singleton=1")
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
    let reference = BrokerControllerRef {
        id: row.try_get("id").map_err(storage_error)?,
        generation: row.try_get("generation").map_err(storage_error)?,
    };
    if Uuid::parse_str(&reference.id).is_err()
        || reference.generation <= 0
        || row
            .try_get::<String, _>("origin_sha256")
            .map_err(storage_error)?
            != origin
    {
        return Err(Error::ExecutionPlanChanged);
    }
    Ok(reference)
}
impl Store {
    pub async fn assert_run_workspace_controller(
        &self,
        spec: &RunSpec,
        origin: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        require_current(&mut tx, spec, Some(origin)).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    /// Trusted current native-broker bootstrap only; an existing different origin
    /// is never silently rebound. Relocation requires explicit migration.
    pub async fn broker_controller(&self, origin: &str) -> Result<BrokerControllerRef> {
        if !cowork_domain::workspace_transfer::valid_sha256(origin) {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query("INSERT INTO broker_controller(singleton,id,generation,origin_sha256) VALUES(1,$1,1,$2) ON CONFLICT(singleton) DO NOTHING")
            .bind(id()).bind(origin).execute(&mut *tx).await.map_err(storage_error)?;
        let result = locked(&mut tx, origin).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
