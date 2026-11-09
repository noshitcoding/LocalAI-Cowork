//! Owner plan custody. Acceptance and the original run ACK share one transaction.
use super::*;
use cowork_workflow_contracts::{
    ExecutionPlan, PreparedExecutionPlan, RunPreflight, RunRequest as PublicRunRequest,
};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredExecutionPlan {
    pub plan: ExecutionPlan,
    pub sha256: String,
    pub request: PublicRunRequest,
    pub spec: RunSpec,
}

pub(super) fn request_fields(request: &PublicRunRequest) -> (&str, &str, i64) {
    match request {
        PublicRunRequest::Single(input) => {
            (&input.order_id, &input.idempotency_key, input.base_revision)
        }
        PublicRunRequest::Workflow(input) => {
            (&input.order_id, &input.idempotency_key, input.base_revision)
        }
    }
}

impl StoredExecutionPlan {
    pub fn validate(&self) -> Result<()> {
        let p = &self.plan;
        let (order, key, _) = request_fields(&self.request);
        if p.format_version != 1
            || p.actor != "installation-owner"
            || p.order_id != order
            || p.idempotency_key != key
            || p.order_id != self.spec.order_id
            || p.project_id != self.spec.project_id
            || p.created_at <= 0
            || p.expires_at.checked_sub(p.created_at) != Some(300_000)
            || p.request_sha256 != canonical_hash(&self.request)
            || p.spec_sha256 != canonical_hash(&self.spec)
            || self.sha256 != canonical_hash(p)
            || p.boundary.executor != self.spec.executor
            || p.boundary.environment != self.spec.environment
            || p.boundary.folders != self.spec.folders
            || self.spec.workflow.is_some()
            || self.spec.file_action.is_some()
            || self.spec.input_workspace.is_some()
        {
            return Err(Error::ExecutionPlanChanged);
        }
        valid_text(&p.id, 128)?;
        valid_text(key, 128)?;
        match (&self.request, &p.preflight) {
            (PublicRunRequest::Single(input), RunPreflight::Single(_)) => {
                validate_single_run(input, &self.spec)
            }
            (PublicRunRequest::Workflow(input), RunPreflight::Workflow(flight)) => {
                workflows::product::validate_scope(input, &self.spec)?;
                if flight.workflow.workflow_id != input.workflow_id
                    || flight.workflow.version_id != input.version_id
                {
                    return Err(Error::ExecutionPlanChanged);
                }
                Ok(())
            }
            _ => Err(Error::ExecutionPlanChanged),
        }
    }

    pub fn view(&self, committed_run_id: Option<String>) -> PreparedExecutionPlan {
        PreparedExecutionPlan {
            plan: self.plan.clone(),
            sha256: self.sha256.clone(),
            committed_run_id,
            retired: false,
        }
    }
}

impl Store {
    pub async fn retire_execution_request(
        &self,
        request: &PublicRunRequest,
    ) -> Result<cowork_workflow_contracts::RetiredExecutionRequest> {
        let (order, key, _) = request_fields(request);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let owner: Order = parse(
            &sqlx::query_scalar::<_, String>("SELECT payload FROM orders WHERE id=$1")
                .bind(order)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?,
        )?;
        let identity = serde_json::to_value(request).map_err(storage_error)?;
        workflow_write_keys::lock(
            &mut tx,
            "execution-plan",
            order,
            key,
            &owner.project_id,
            &identity,
        )
        .await?;
        let existing: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM runs WHERE order_id=$1 AND idempotency_key=$2",
        )
        .bind(order)
        .bind(key)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if existing != 0 {
            return Err(Error::Conflict);
        }
        // UPDATE serializes with an original commit; its result is checked again
        // under that lock rather than trusting a previous absence observation.
        sqlx::query("UPDATE owner_execution_plans SET plan_sha256=plan_sha256 WHERE order_id=$1 AND idempotency_key=$2")
            .bind(order).bind(key).execute(&mut *tx).await.map_err(storage_error)?;
        let accepted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM owner_execution_plans WHERE order_id=$1 AND idempotency_key=$2 AND run_id IS NOT NULL")
            .bind(order).bind(key).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if accepted != 0 {
            return Err(Error::Conflict);
        }
        sqlx::query("UPDATE owner_execution_plans SET retired=1 WHERE order_id=$1 AND idempotency_key=$2 AND run_id IS NULL")
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

    pub async fn execution_plan_retired(&self, prepared: &StoredExecutionPlan) -> Result<bool> {
        let retired: i64 = sqlx::query_scalar(
            "SELECT retired FROM owner_execution_plans WHERE id=$1 AND plan_sha256=$2",
        )
        .bind(&prepared.plan.id)
        .bind(&prepared.sha256)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        Ok(retired == 1)
    }

    /// Serializes against admission, so an in-flight late commit cannot race a
    /// user's discard into a second run under a newly generated request key.
    pub async fn retire_execution_plan(&self, prepared: &StoredExecutionPlan) -> Result<()> {
        prepared.validate()?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query("UPDATE owner_execution_plans SET plan_sha256=plan_sha256 WHERE id=$1 AND plan_sha256=$2")
            .bind(&prepared.plan.id).bind(&prepared.sha256).execute(&mut *tx).await.map_err(storage_error)?;
        let row = sqlx::query("SELECT run_id,plan_sha256 FROM owner_execution_plans WHERE id=$1")
            .bind(&prepared.plan.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        if row
            .try_get::<Option<String>, _>("run_id")
            .map_err(storage_error)?
            .is_some()
            || row
                .try_get::<String, _>("plan_sha256")
                .map_err(storage_error)?
                != prepared.sha256
        {
            return Err(Error::Conflict);
        }
        sqlx::query("UPDATE owner_execution_plans SET retired=1 WHERE id=$1 AND run_id IS NULL")
            .bind(&prepared.plan.id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }

    pub async fn existing_execution_plan(
        &self,
        request: &PublicRunRequest,
    ) -> Result<Option<StoredExecutionPlan>> {
        let (order, key, _) = request_fields(request);
        valid_text(key, 128)?;
        let row = sqlx::query("SELECT request_sha256,payload FROM owner_execution_plans WHERE order_id=$1 AND idempotency_key=$2")
            .bind(order).bind(key).fetch_optional(&self.pool).await.map_err(storage_error)?;
        let Some(row) = row else { return Ok(None) };
        if row
            .try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != canonical_hash(request)
        {
            return Err(Error::Conflict);
        }
        let saved: StoredExecutionPlan =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        saved.validate()?;
        Ok(Some(saved))
    }

    pub async fn retain_execution_plan(
        &self,
        prepared: &StoredExecutionPlan,
    ) -> Result<StoredExecutionPlan> {
        self.retain_execution_plan_with_target(prepared, None).await
    }

    pub async fn retain_execution_plan_with_target(
        &self,
        prepared: &StoredExecutionPlan,
        target: Option<&ExecutionTargetRef>,
    ) -> Result<StoredExecutionPlan> {
        prepared.validate()?;
        let p = &prepared.plan;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        if prepared.spec.environment.is_some() {
            let target = target.ok_or(Error::ExecutionPlanChanged)?;
            let registered =
                execution_targets::check_reference_in_transaction(&mut tx, target, false).await?;
            if registered.identity.runner_id != prepared.spec.executor {
                return Err(Error::ExecutionPlanChanged);
            }
        } else if target.is_some() {
            return Err(Error::ExecutionPlanChanged);
        }
        workflow_drafts::require_project(&mut tx, &p.project_id).await?;
        let identity = serde_json::to_value(&prepared.request).map_err(storage_error)?;
        let reservation = workflow_write_keys::lock(
            &mut tx,
            "execution-plan",
            &p.order_id,
            &p.idempotency_key,
            &p.project_id,
            &identity,
        )
        .await?;
        workflow_write_keys::permit(&reservation, None)?;
        let order: Order = parse(
            &sqlx::query_scalar::<_, String>("SELECT payload FROM orders WHERE id=$1")
                .bind(&p.order_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?,
        )?;
        if order.project_id != p.project_id {
            return Err(Error::NotFound);
        }
        if order.revision != request_fields(&prepared.request).2 {
            return Err(Error::Conflict);
        }
        let inserted=sqlx::query("INSERT INTO owner_execution_plans(id,project_id,order_id,idempotency_key,request_sha256,plan_sha256,payload) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(order_id,idempotency_key) DO NOTHING")
            .bind(&p.id).bind(&p.project_id).bind(&p.order_id).bind(&p.idempotency_key)
            .bind(&p.request_sha256).bind(&prepared.sha256).bind(json_string(prepared)?)
            .execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if inserted == 1
            && let Some(target) = target
        {
            sqlx::query("INSERT INTO owner_execution_plan_targets(plan_id,target_id,target_revision,identity_sha256) VALUES($1,$2,$3,$4)")
                .bind(&p.id).bind(&target.target_id).bind(target.revision).bind(&target.identity_sha256)
                .execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        self.existing_execution_plan(&prepared.request)
            .await?
            .ok_or(Error::Conflict)
    }

    pub async fn execution_plan(&self, order: &str, plan: &str) -> Result<StoredExecutionPlan> {
        let row = sqlx::query(
            "SELECT plan_sha256,payload FROM owner_execution_plans WHERE id=$1 AND order_id=$2",
        )
        .bind(plan)
        .bind(order)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let saved: StoredExecutionPlan =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        saved.validate()?;
        if saved.plan.id != plan
            || saved.plan.order_id != order
            || saved.sha256
                != row
                    .try_get::<String, _>("plan_sha256")
                    .map_err(storage_error)?
        {
            return Err(Error::ExecutionPlanChanged);
        }
        Ok(saved)
    }

    /// The exact initial response remains available after completion or profile removal.
    pub async fn execution_plan_run(
        &self,
        prepared: &StoredExecutionPlan,
    ) -> Result<Option<RunRecord>> {
        prepared.validate()?;
        let row = sqlx::query("SELECT plan_sha256,run_id,receipt FROM owner_execution_plans WHERE id=$1 AND order_id=$2")
            .bind(&prepared.plan.id).bind(&prepared.plan.order_id).fetch_optional(&self.pool).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        if row
            .try_get::<String, _>("plan_sha256")
            .map_err(storage_error)?
            != prepared.sha256
        {
            return Err(Error::ExecutionPlanChanged);
        }
        read_receipt(&row, prepared)
    }

    pub async fn create_execution_plan_run(
        &self,
        prepared: &StoredExecutionPlan,
    ) -> Result<RunRecord> {
        prepared.validate()?;
        let (request, hash, identity) = match &prepared.request {
            PublicRunRequest::Single(input) => (
                RunRequest {
                    workflow: None,
                    order_id: input.order_id.clone(),
                    base_revision: input.base_revision,
                    idempotency_key: input.idempotency_key.clone(),
                    prompt: input.prompt.clone(),
                },
                canonical_hash(input),
                json!({"kind":"single","request":input}),
            ),
            PublicRunRequest::Workflow(input) => {
                let RunPreflight::Workflow(flight) = &prepared.plan.preflight else {
                    return Err(Error::ExecutionPlanChanged);
                };
                (
                    RunRequest {
                        workflow: Some(workflows::product::start(
                            input,
                            Some(flight.workflow.clone()),
                        )),
                        order_id: input.order_id.clone(),
                        base_revision: input.base_revision,
                        idempotency_key: input.idempotency_key.clone(),
                        prompt: input.prompt.clone(),
                    },
                    workflows::product::request_hash(input),
                    crate::workflow_operations::action_identity(
                        &prepared.spec.project_id,
                        &cowork_workflow_contracts::WorkflowOperationRequest::Start {
                            request: input.clone().into(),
                        },
                    ),
                )
            }
        };
        self.insert_run_with_plan(
            &request,
            prepared.spec.clone(),
            hash,
            false,
            identity,
            Some(prepared),
        )
        .await
    }
}

fn read_receipt(
    row: &sqlx::any::AnyRow,
    prepared: &StoredExecutionPlan,
) -> Result<Option<RunRecord>> {
    let run: Option<String> = row.try_get("run_id").map_err(storage_error)?;
    let receipt: Option<String> = row.try_get("receipt").map_err(storage_error)?;
    match (run, receipt) {
        (None, None) => Ok(None),
        (Some(id), Some(payload)) => {
            let record: RunRecord = parse(&payload)?;
            if record.id != id
                || record.spec.order_id != prepared.plan.order_id
                || record.spec.project_id != prepared.plan.project_id
            {
                return Err(Error::ExecutionPlanChanged);
            }
            Ok(Some(record))
        }
        _ => Err(Error::ExecutionPlanChanged),
    }
}

pub(super) async fn lock(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    prepared: &StoredExecutionPlan,
) -> Result<Option<RunRecord>> {
    prepared.validate()?;
    sqlx::query(
        "UPDATE owner_execution_plans SET plan_sha256=plan_sha256 WHERE id=$1 AND order_id=$2",
    )
    .bind(&prepared.plan.id)
    .bind(&prepared.plan.order_id)
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?;
    let row = sqlx::query("SELECT plan_sha256,payload,run_id,receipt,retired FROM owner_execution_plans WHERE id=$1 AND order_id=$2")
        .bind(&prepared.plan.id).bind(&prepared.plan.order_id).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let saved: StoredExecutionPlan =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    saved.validate()?;
    if row
        .try_get::<String, _>("plan_sha256")
        .map_err(storage_error)?
        != prepared.sha256
        || saved.sha256 != prepared.sha256
    {
        return Err(Error::ExecutionPlanChanged);
    }
    if let Some(receipt) = read_receipt(&row, prepared)? {
        return Ok(Some(receipt));
    }
    if row.try_get::<i64, _>("retired").map_err(storage_error)? != 0 {
        return Err(Error::ExecutionPlanChanged);
    }
    let target=sqlx::query("SELECT target_id,target_revision,identity_sha256 FROM owner_execution_plan_targets WHERE plan_id=$1")
        .bind(&prepared.plan.id).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    match (prepared.spec.environment.is_some(), target) {
        (false, None) => {}
        (true, Some(row)) => {
            let reference = ExecutionTargetRef {
                target_id: row.try_get("target_id").map_err(storage_error)?,
                revision: row.try_get("target_revision").map_err(storage_error)?,
                identity_sha256: row.try_get("identity_sha256").map_err(storage_error)?,
            };
            let registered =
                execution_targets::check_reference_in_transaction(tx, &reference, true).await?;
            if registered.identity.runner_id != prepared.spec.executor {
                return Err(Error::ExecutionPlanChanged);
            }
        }
        _ => return Err(Error::ExecutionPlanChanged),
    }
    let at = now();
    if at < prepared.plan.created_at || at >= prepared.plan.expires_at {
        return Err(Error::ExecutionPlanExpired);
    }
    Ok(None)
}

pub(super) async fn accept(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    prepared: &StoredExecutionPlan,
    run: &RunRecord,
) -> Result<()> {
    if sqlx::query("UPDATE owner_execution_plans SET run_id=$1,receipt=$2 WHERE id=$3 AND plan_sha256=$4 AND run_id IS NULL")
        .bind(&run.id).bind(json_string(run)?).bind(&prepared.plan.id).bind(&prepared.sha256)
        .execute(&mut **tx).await.map_err(storage_error)?.rows_affected() != 1 { return Err(Error::Conflict) }
    Ok(())
}

pub(super) fn validate_single_run(input: &CreateRun, spec: &RunSpec) -> Result<()> {
    let selected: std::collections::BTreeSet<_> =
        input.native_tools.iter().map(String::as_str).collect();
    let selected: std::collections::BTreeSet<_> = selected
        .into_iter()
        .filter(|tool| {
            !matches!(input.access_mode, AccessMode::ReadOnly)
                && input
                    .agent_config
                    .as_ref()
                    .and_then(|c| c.tool_filter.as_ref())
                    .is_none_or(|filter| filter.iter().any(|id| id == tool))
        })
        .collect();
    let frozen: std::collections::BTreeSet<_> = spec
        .allowed_tools
        .iter()
        .map(String::as_str)
        .filter(|name| {
            matches!(*name, "execute_program" | "browser" | "office" | "github")
                && !(spec.environment.is_some() && *name == "execute_program")
        })
        .collect();
    if input
        .native_tools
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != input.native_tools.len()
        || selected != frozen
        || (!selected.is_empty()
            && (spec.environment.is_some() || spec.executor != "owner-native-toolbox-v1"))
    {
        return Err(Error::Forbidden);
    }
    let selections: Vec<_> = spec
        .mcp_tools
        .iter()
        .map(|b| McpToolSelection {
            server_id: b.server_id.clone(),
            server_revision: b.server_revision,
            tool_name: b.definition["name"].as_str().unwrap_or_default().into(),
        })
        .collect();
    let requested_mcp: Vec<_> = input
        .mcp_tools
        .iter()
        .filter(|tool| {
            !matches!(input.access_mode, AccessMode::ReadOnly)
                && input
                    .agent_config
                    .as_ref()
                    .and_then(|c| c.tool_filter.as_ref())
                    .is_none_or(|filter| {
                        filter.contains(&cowork_domain::mcp::mcp_alias(
                            &tool.server_id,
                            &tool.tool_name,
                        ))
                    })
        })
        .cloned()
        .collect();
    if selections != requested_mcp || spec.budgets != input.budgets {
        return Err(Error::Conflict);
    }
    if spec.provider.is_none() || spec.file_action.is_some() || spec.workflow.is_some() {
        return Err(Error::Forbidden);
    }
    Ok(())
}
