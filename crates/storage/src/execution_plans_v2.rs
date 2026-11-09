//! Shared durable run creation with a confirmed snapshot and private workspace.
use super::*;
use cowork_domain::{execution_inputs::CapturedExecutionInput, run_workspaces::RunWorkspaceIntent};
use cowork_workflow_contracts::{
    ExecutionDestinationV2, ExecutionNetworkAccess, ExecutionPlanV2, ExecutionPreflightV2,
    ExecutionProcessAccess, PrepareOwnerExecutionPlanV2, PreparedExecutionPlanV2, RunPreflight,
    RunRequest as PublicRequest,
};
pub(super) mod documents;
mod imports;
pub(super) mod managed;
mod requests;
pub use requests::StoredExecutionRequestV2;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredExecutionPlanV2 {
    pub plan: ExecutionPlanV2,
    pub sha256: String,
    pub request: StoredExecutionRequestV2,
    pub source_spec: RunSpec,
    pub source_origins: Vec<FolderGrant>,
    pub spec: RunSpec,
    pub workspace: RunWorkspaceIntent,
}

pub fn broker_tools(spec: &RunSpec) -> bool {
    cowork_domain::run_workspaces::broker_scope(spec)
}

fn fields(request: &PublicRequest) -> (&str, &str, i64, &str) {
    match request {
        PublicRequest::Single(r) => (&r.order_id, &r.idempotency_key, r.base_revision, &r.prompt),
        PublicRequest::Workflow(r) => (&r.order_id, &r.idempotency_key, r.base_revision, &r.prompt),
    }
}

impl StoredExecutionPlanV2 {
    pub fn capture(&self) -> CapturedExecutionInput {
        CapturedExecutionInput {
            snapshot: self.plan.input.clone(),
            sha256: self.request.input().sha256.clone(),
            origins: self.source_origins.clone(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        let p = &self.plan;
        let (order, key, _, _) = self.request.fields();
        let capture = self.capture();
        capture.validate()?;
        if p.format_version != 2
            || p.actor != (ExecutionActor::InstallationOwner {})
            || p.order_id != order
            || p.idempotency_key != key
            || p.project_id != self.source_spec.project_id
            || p.order_id != self.source_spec.order_id
            || p.created_at <= 0
            || p.expires_at.checked_sub(p.created_at) != Some(300_000)
            || p.request_sha256 != canonical_hash(&self.request)
            || self.sha256 != canonical_hash(p)
            || p.spec_sha256 != canonical_hash(&self.spec)
            || self.source_spec.input_workspace.is_some()
            || !self.destination_scope()
            || p.workspace != self.workspace.binding
            || self.spec.input_workspace.as_ref() != Some(&p.workspace)
            || self.request.input() != &p.workspace.input
            || self.request.input().snapshot_id != p.input.id
            || self.request.input().manifest_sha256 != p.input.manifest.sha256
            || p.boundary.folders != self.spec.folders
            || p.boundary.executor != self.spec.executor
            || !p.boundary.input_snapshot_bound
            || !p.boundary.connector_aliases.is_empty()
        {
            return Err(Error::ExecutionPlanChanged);
        }
        let expected_intent = RunWorkspaceIntent::for_spec(
            &capture,
            &self.source_spec,
            &self.workspace.binding.id,
            &self.workspace.nonce,
        )?;
        if expected_intent != self.workspace {
            return Err(Error::ExecutionPlanChanged);
        }
        let mut projected = self.source_spec.clone();
        projected.input_workspace = Some(self.workspace.binding.clone());
        projected.folders = self.workspace.folders();
        if canonical_hash(&projected) != canonical_hash(&self.spec) {
            return Err(Error::ExecutionPlanChanged);
        }
        match (
            self.request.owner().map(|value| &value.request),
            &p.preflight,
        ) {
            (
                Some(PublicRequest::Single(request)),
                ExecutionPreflightV2::Owner {
                    review: RunPreflight::Single(flight),
                },
            ) => {
                execution_plans::validate_single_run(request, &self.source_spec)?;
                if flight.folders != self.spec.folders
                    || flight.allowed_tools != self.spec.allowed_tools
                    || flight.executor != self.spec.executor
                    || flight.access_mode != self.spec.access_mode
                    || flight.budgets != self.spec.budgets
                    || self
                        .spec
                        .provider
                        .as_ref()
                        .is_none_or(|v| canonical_hash(v) != canonical_hash(&flight.provider))
                {
                    return Err(Error::ExecutionPlanChanged);
                }
            }
            (
                Some(PublicRequest::Workflow(request)),
                ExecutionPreflightV2::Owner {
                    review: RunPreflight::Workflow(flight),
                },
            ) => {
                workflows::product::validate_scope(request, &self.source_spec)?;
                if flight.workflow.workflow_id != request.workflow_id
                    || flight.workflow.version_id != request.version_id
                    || flight.input_source != request.input_source
                    || flight.allowed_tools != self.spec.allowed_tools
                    || flight.access_mode != self.spec.access_mode
                {
                    return Err(Error::ExecutionPlanChanged);
                }
                if let Some(source) = &request.input_source {
                    let inputs = flight
                        .workflow
                        .inputs
                        .as_ref()
                        .ok_or(Error::ExecutionPlanChanged)?;
                    if request.inputs.is_some()
                        || request.expected_binding.is_some()
                        || source.run_id.is_empty()
                        || source.sha256 != canonical_hash(inputs)
                    {
                        return Err(Error::ExecutionPlanChanged);
                    }
                }
            }
            (None, ExecutionPreflightV2::Restore { review }) => {
                self.request.validate_restore(self, review)?
            }
            (None, ExecutionPreflightV2::Import { review }) => {
                self.request.validate_import(self, review)?
            }
            _ => return Err(Error::ExecutionPlanChanged),
        }
        Ok(())
    }
    pub fn view(&self, run: Option<String>, retired: bool) -> PreparedExecutionPlanV2 {
        PreparedExecutionPlanV2 {
            plan: self.plan.clone(),
            canonical_json: serde_json::to_string(
                &serde_json::to_value(&self.plan).expect("validated public plan"),
            )
            .expect("validated public plan JSON"),
            sha256: self.sha256.clone(),
            committed_run_id: run,
            retired,
        }
    }
}

pub(super) fn checked(row: &sqlx::any::AnyRow) -> Result<StoredExecutionPlanV2> {
    let saved: StoredExecutionPlanV2 =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    saved.validate()?;
    if saved.plan.id != row.try_get::<String, _>("id").map_err(storage_error)?
        || saved.plan.order_id
            != row
                .try_get::<String, _>("order_id")
                .map_err(storage_error)?
        || saved.plan.project_id
            != row
                .try_get::<String, _>("project_id")
                .map_err(storage_error)?
        || saved.plan.idempotency_key
            != row
                .try_get::<String, _>("idempotency_key")
                .map_err(storage_error)?
        || saved.sha256
            != row
                .try_get::<String, _>("plan_sha256")
                .map_err(storage_error)?
        || canonical_hash(&saved)
            != row
                .try_get::<String, _>("custody_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::ExecutionPlanChanged);
    }
    Ok(saved)
}
fn receipt(row: &sqlx::any::AnyRow, saved: &StoredExecutionPlanV2) -> Result<Option<RunRecord>> {
    match (
        row.try_get::<Option<String>, _>("run_id")
            .map_err(storage_error)?,
        row.try_get::<Option<String>, _>("receipt")
            .map_err(storage_error)?,
    ) {
        (None, None) => Ok(None),
        (Some(id), Some(payload)) => {
            let run: RunRecord = parse(&payload)?;
            if run.id != id
                || run.spec.input_workspace != saved.spec.input_workspace
                || run.spec.order_id != saved.plan.order_id
                || run.spec.project_id != saved.plan.project_id
            {
                return Err(Error::ExecutionPlanChanged);
            }
            Ok(Some(run))
        }
        _ => Err(Error::ExecutionPlanChanged),
    }
}

impl Store {
    pub async fn existing_execution_plan_v2(
        &self,
        request: &PrepareOwnerExecutionPlanV2,
    ) -> Result<Option<StoredExecutionPlanV2>> {
        self.existing_execution_plan_v2_request(&request.clone().into())
            .await
    }
    pub async fn existing_execution_plan_v2_request(
        &self,
        request: &StoredExecutionRequestV2,
    ) -> Result<Option<StoredExecutionPlanV2>> {
        let (order, key, _, _) = request.fields();
        let row = sqlx::query(
            "SELECT * FROM owner_execution_plans_v2 WHERE order_id=$1 AND idempotency_key=$2",
        )
        .bind(order)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        row.map(|row| {
            let saved = checked(&row)?;
            if saved.plan.request_sha256 != canonical_hash(request) {
                return Err(Error::Conflict);
            }
            Ok(saved)
        })
        .transpose()
    }
    pub async fn retain_execution_plan_v2(
        &self,
        saved: &StoredExecutionPlanV2,
    ) -> Result<StoredExecutionPlanV2> {
        saved.validate()?;
        let p = &saved.plan;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let key = workflow_write_keys::lock(
            &mut tx,
            "execution-plan",
            &p.order_id,
            &p.idempotency_key,
            &p.project_id,
            &serde_json::to_value(&saved.request).map_err(storage_error)?,
        )
        .await?;
        workflow_write_keys::permit(&key, None)?;
        sqlx::query("INSERT INTO owner_execution_plans_v2(id,order_id,project_id,idempotency_key,plan_sha256,custody_sha256,payload) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(order_id,idempotency_key) DO NOTHING")
   .bind(&p.id).bind(&p.order_id).bind(&p.project_id).bind(&p.idempotency_key).bind(&saved.sha256).bind(canonical_hash(saved)).bind(json_string(saved)?).execute(&mut *tx).await.map_err(storage_error)?;
        let row = sqlx::query(
            "SELECT * FROM owner_execution_plans_v2 WHERE order_id=$1 AND idempotency_key=$2",
        )
        .bind(&p.order_id)
        .bind(&p.idempotency_key)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let original = checked(&row)?;
        if original.plan.request_sha256 != p.request_sha256 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(original)
    }
    pub async fn execution_plan_v2(&self, order: &str, id: &str) -> Result<StoredExecutionPlanV2> {
        let row = sqlx::query("SELECT * FROM owner_execution_plans_v2 WHERE order_id=$1 AND id=$2")
            .bind(order)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        checked(&row)
    }
    pub async fn execution_plan_v2_view(
        &self,
        saved: &StoredExecutionPlanV2,
    ) -> Result<PreparedExecutionPlanV2> {
        let row = sqlx::query("SELECT * FROM owner_execution_plans_v2 WHERE id=$1")
            .bind(&saved.plan.id)
            .fetch_one(&self.pool)
            .await
            .map_err(storage_error)?;
        let original = checked(&row)?;
        if original.sha256 != saved.sha256 {
            return Err(Error::ExecutionPlanChanged);
        }
        Ok(saved.view(
            receipt(&row, saved)?.map(|r| r.id),
            row.try_get::<i64, _>("retired").map_err(storage_error)? != 0,
        ))
    }
    pub async fn execution_plan_v2_receipt(
        &self,
        saved: &StoredExecutionPlanV2,
    ) -> Result<Option<RunRecord>> {
        let row = sqlx::query("SELECT * FROM owner_execution_plans_v2 WHERE id=$1")
            .bind(&saved.plan.id)
            .fetch_one(&self.pool)
            .await
            .map_err(storage_error)?;
        if checked(&row)?.sha256 != saved.sha256 {
            return Err(Error::ExecutionPlanChanged);
        }
        receipt(&row, saved)
    }
    pub async fn create_execution_plan_v2_run(
        &self,
        saved: &StoredExecutionPlanV2,
        origin: &str,
        workspace: &Value,
    ) -> Result<RunRecord> {
        saved.validate()?;
        let p = &saved.plan;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let money = model_money::lock_creation(&mut tx).await?;
        workflow_drafts::require_project(&mut tx, &p.project_id).await?;
        let prepare_key = workflow_write_keys::lock(
            &mut tx,
            "execution-plan",
            &p.order_id,
            &p.idempotency_key,
            &p.project_id,
            &serde_json::to_value(&saved.request).map_err(storage_error)?,
        )
        .await?;
        workflow_write_keys::permit(&prepare_key, None)?;
        let identity = json!({"kind":"execution_plan_v2","request":saved.request});
        let reservation = workflow_write_keys::lock(
            &mut tx,
            "run",
            &p.order_id,
            &p.idempotency_key,
            &p.project_id,
            &identity,
        )
        .await?;
        workflow_write_keys::permit(&reservation, None)?;
        sqlx::query("UPDATE owner_execution_plans_v2 SET retired=retired WHERE id=$1")
            .bind(&p.id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let row = sqlx::query("SELECT * FROM owner_execution_plans_v2 WHERE id=$1")
            .bind(&p.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        if checked(&row)?.sha256 != saved.sha256 {
            return Err(Error::ExecutionPlanChanged);
        }
        if let Some(existing) = receipt(&row, saved)? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(existing);
        }
        if row.try_get::<i64, _>("retired").map_err(storage_error)? != 0
            || now() < p.created_at
            || now() >= p.expires_at
        {
            return Err(Error::ExecutionPlanExpired);
        }
        if execution_controllers::locked(&mut tx, origin).await? != *p.target.controller() {
            return Err(Error::ExecutionPlanChanged);
        }
        managed::require_current(&mut tx, saved).await?;
        sqlx::query(
            "UPDATE owner_execution_inputs SET retired=retired WHERE id=$1 AND order_id=$2",
        )
        .bind(&p.input.id)
        .bind(&p.order_id)
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        let row = sqlx::query("SELECT * FROM owner_execution_inputs WHERE id=$1 AND order_id=$2")
            .bind(&p.input.id)
            .bind(&p.order_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let input = execution_inputs::checked(&row, &p.order_id)?;
        if input.retired || canonical_hash(&input.capture) != canonical_hash(&saved.capture()) {
            return Err(Error::ExecutionPlanChanged);
        }
        if workspace["intent"] != serde_json::to_value(&saved.workspace).map_err(storage_error)? {
            return Err(Error::ExecutionPlanChanged);
        }
        let (_, _, base, prompt) = saved.request.fields();
        let workflow = match (
            saved.request.owner().map(|value| &value.request),
            &p.preflight,
        ) {
            (
                Some(PublicRequest::Workflow(request)),
                ExecutionPreflightV2::Owner {
                    review: RunPreflight::Workflow(flight),
                },
            ) => {
                let mut resolved = request.clone();
                if request.input_source.is_some() {
                    resolved.inputs = flight.workflow.inputs.clone();
                }
                Some(workflows::product::start(
                    &resolved,
                    Some(flight.workflow.clone()),
                ))
            }
            _ => None,
        };
        let request = RunRequest {
            workflow,
            order_id: p.order_id.clone(),
            base_revision: base,
            idempotency_key: p.idempotency_key.clone(),
            prompt: prompt.into(),
        };
        let (record, created) = self
            .insert_run_in_tx(
                &mut tx,
                &money,
                &request,
                saved.spec.clone(),
                p.request_sha256.clone(),
                !matches!(saved.request, StoredExecutionRequestV2::Owner(_)),
            )
            .await?;
        if !created {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO owner_run_workspaces(workspace_id,plan_id,run_id,custody_sha256,payload) VALUES($1,$2,$3,$4,$5)")
   .bind(&p.workspace.id).bind(&p.id).bind(&record.id).bind(canonical_hash(workspace)).bind(json_string(workspace)?).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE owner_execution_plans_v2 SET run_id=$1,receipt=$2 WHERE id=$3 AND run_id IS NULL").bind(&record.id).bind(json_string(&record)?).bind(&p.id).execute(&mut *tx).await.map_err(storage_error)?;
        let ack = cowork_workflow_contracts::WorkflowOperationReceipt::Started {
            run_id: record.id.clone(),
            spec: Box::new(record.spec.clone()),
            created_at: record.created_at,
        };
        workflow_write_keys::accept(
            &mut tx,
            "run",
            &p.order_id,
            &p.idempotency_key,
            &serde_json::to_value(ack).map_err(storage_error)?,
        )
        .await?;
        if record.spec.workflow.is_some() {
            self.workflow_clock
                .assert_creation_time(&mut tx, &record)
                .await?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(record)
    }
    pub async fn run_workspace_custody(&self, binding: &RunWorkspaceBinding) -> Result<Value> {
        let row = sqlx::query(
            "SELECT payload,custody_sha256 FROM owner_run_workspaces WHERE workspace_id=$1",
        )
        .bind(&binding.id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let payload: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if canonical_hash(&payload)
            != row
                .try_get::<String, _>("custody_sha256")
                .map_err(storage_error)?
            || payload["intent"]["binding"]
                != serde_json::to_value(binding).map_err(storage_error)?
        {
            return Err(Error::ExecutionPlanChanged);
        }
        Ok(payload)
    }
    pub async fn retire_execution_plan_v2(&self, saved: &StoredExecutionPlanV2) -> Result<()> {
        saved.validate()?;
        let p = &saved.plan;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        workflow_write_keys::lock(
            &mut tx,
            "execution-plan",
            &p.order_id,
            &p.idempotency_key,
            &p.project_id,
            &serde_json::to_value(&saved.request).map_err(storage_error)?,
        )
        .await?;
        sqlx::query("UPDATE owner_execution_plans_v2 SET retired=retired WHERE id=$1")
            .bind(&p.id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let row = sqlx::query("SELECT * FROM owner_execution_plans_v2 WHERE id=$1")
            .bind(&p.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        if checked(&row)?.sha256 != saved.sha256 || receipt(&row, saved)?.is_some() {
            return Err(Error::Conflict);
        }
        sqlx::query("UPDATE owner_execution_plans_v2 SET retired=1 WHERE id=$1")
            .bind(&p.id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        sqlx::query("UPDATE workflow_write_keys SET state='discarded' WHERE namespace='execution-plan' AND target_id=$1 AND request_key=$2").bind(&p.order_id).bind(&p.idempotency_key).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }

    /// Fence the exact original request even when its prepare ACK was lost.
    pub async fn retire_execution_request_v2(
        &self,
        request: &PrepareOwnerExecutionPlanV2,
    ) -> Result<cowork_workflow_contracts::RetiredExecutionRequest> {
        self.retire_execution_plan_v2_request(&request.clone().into())
            .await
    }
    pub async fn retire_execution_plan_v2_request(
        &self,
        request: &StoredExecutionRequestV2,
    ) -> Result<cowork_workflow_contracts::RetiredExecutionRequest> {
        let (order, key, _, _) = request.fields();
        let owner = self.order(order).await?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        workflow_write_keys::lock(
            &mut tx,
            "execution-plan",
            order,
            key,
            &owner.project_id,
            &serde_json::to_value(request).map_err(storage_error)?,
        )
        .await?;
        sqlx::query("UPDATE owner_execution_plans_v2 SET retired=retired WHERE order_id=$1 AND idempotency_key=$2")
            .bind(order).bind(key).execute(&mut *tx).await.map_err(storage_error)?;
        if let Some(row) = sqlx::query(
            "SELECT * FROM owner_execution_plans_v2 WHERE order_id=$1 AND idempotency_key=$2",
        )
        .bind(order)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        {
            let saved = checked(&row)?;
            if saved.plan.request_sha256 != canonical_hash(request)
                || receipt(&row, &saved)?.is_some()
            {
                return Err(Error::Conflict);
            }
        }
        // A legacy/internally accepted run with the same order/key cannot be
        // discarded by a different contract version.
        let runs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM runs WHERE order_id=$1 AND idempotency_key=$2",
        )
        .bind(order)
        .bind(key)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if runs != 0 {
            return Err(Error::Conflict);
        }
        sqlx::query("UPDATE owner_execution_plans_v2 SET retired=1 WHERE order_id=$1 AND idempotency_key=$2 AND run_id IS NULL")
            .bind(order).bind(key).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE workflow_write_keys SET state='discarded' WHERE namespace='execution-plan' AND target_id=$1 AND request_key=$2")
            .bind(order).bind(key).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(cowork_workflow_contracts::RetiredExecutionRequest {
            order_id: order.into(),
            idempotency_key: key.into(),
            request_sha256: canonical_hash(request),
            retired: true,
        })
    }
}
