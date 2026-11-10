//! Local custody for a confirmed plan's one existing process attempt. Public
//! bodies, historical receipts and copied launch labels cannot create authority.
use super::*;
use cowork_domain::environments::{EnvironmentObservation, EnvironmentPin};
use cowork_runner_protocol::{Binding, LaunchRequest, ManagedInputGrant, Signed};
use cowork_workflow_contracts::ExecutionDestinationV2;
use sqlx::{Any, Transaction};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedInputTransfer {
    pub request: Signed<LaunchRequest>,
    pub grant: Signed<ManagedInputGrant>,
    pub manifest: WorkspaceTransferManifest,
    /// Retained portable entries, local to the controller's content store.
    pub snapshot: WorkspaceSnapshot,
}

pub struct ManagedInputContext {
    pub plan: StoredExecutionPlanV2,
    pub run: RunRecord,
    pub workspace: Value,
}

pub(super) fn binding(identity: &ExecutionRunnerIdentity) -> Binding {
    Binding {
        runner_id: identity.runner_id.clone(),
        generation: identity.generation,
        adapter_build_sha256: identity.adapter_build_sha256.clone(),
        control_public_key: identity.control_public_key.clone(),
        runner_public_key: identity.runner_public_key.clone(),
    }
}

async fn context(
    tx: &mut Transaction<'_, Any>,
    token: &str,
    request: &Signed<LaunchRequest>,
    observation: &EnvironmentObservation,
) -> Result<ManagedInputContext> {
    let r = &request.payload;
    let row = sqlx::query("SELECT revision,payload FROM runs WHERE id=$1")
        .bind(&r.run_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    let run: StoredRun = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    run.assert_lease(token, now())?;
    if run.record.id != r.run_id
        || run.record.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || run.record.state != RunState::Running
        || run.record.spec.workflow.is_some()
        || r.worker_fence_sha256 != hash(token.as_bytes())
        || r.lease_epoch > run.record.revision as u64
        || r.expires_at > run.lease.as_ref().ok_or(Error::StaleLease)?.until
    {
        return Err(Error::Forbidden);
    }
    let row = sqlx::query("SELECT p.*,w.payload AS workspace_payload,w.custody_sha256 AS workspace_sha256,w.workspace_id AS bound_workspace,w.run_id AS bound_run FROM owner_run_workspaces w JOIN owner_execution_plans_v2 p ON p.id=w.plan_id AND p.run_id=w.run_id WHERE w.run_id=$1")
        .bind(&r.run_id).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::ExecutionPlanChanged)?;
    let plan = execution_plans_v2::checked(&row)?;
    let accepted: RunRecord = parse(
        &row.try_get::<Option<String>, _>("receipt")
            .map_err(storage_error)?
            .ok_or(Error::ExecutionPlanChanged)?,
    )?;
    if row.try_get::<i64, _>("retired").map_err(storage_error)? != 0
        || accepted.id != run.record.id
        || accepted.spec.input_workspace != run.record.spec.input_workspace
        || canonical_hash(&accepted.spec) != plan.plan.spec_sha256
        || canonical_hash(&run.record.spec) != plan.plan.spec_sha256
        || row
            .try_get::<String, _>("bound_run")
            .map_err(storage_error)?
            != run.record.id
        || row
            .try_get::<String, _>("bound_workspace")
            .map_err(storage_error)?
            != plan.workspace.binding.id
    {
        return Err(Error::ExecutionPlanChanged);
    }
    let ExecutionDestinationV2::ManagedRunner {
        target,
        runner,
        programs,
        ..
    } = &plan.plan.target
    else {
        return Err(Error::Forbidden);
    };
    request
        .verify("launch", &runner.control_public_key)
        .map_err(|_| Error::Forbidden)?;
    let workspace: Value = parse(
        &row.try_get::<String, _>("workspace_payload")
            .map_err(storage_error)?,
    )?;
    if canonical_hash(&workspace)
        != row
            .try_get::<String, _>("workspace_sha256")
            .map_err(storage_error)?
        || workspace["intent"] != serde_json::to_value(&plan.workspace).map_err(storage_error)?
    {
        return Err(Error::ExecutionPlanChanged);
    }
    execution_controllers::require_current(tx, &run.record.spec, None).await?;
    let row = sqlx::query(
        "SELECT id,tool_call_id,revision,payload FROM process_attempts WHERE id=$1 AND run_id=$2",
    )
    .bind(&r.process_attempt_id)
    .bind(&r.run_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .ok_or(Error::Forbidden)?;
    let attempt = processes::checked_record(&row, &r.run_id)?;
    let p = &attempt.record;
    if p.id != r.process_attempt_id
        || p.tool.id != r.tool_call_id
        || p.request_sha256 != r.request_sha256
        || p.state != ProcessAttemptState::LaunchCommitted
        || attempt.lease_sha256 != hash(token.as_bytes())
        || p.environment != r.environment
        || p.workspace_resource_id != r.workspace_resource_id
        || run.checkpoint.in_flight.as_ref() != Some(&p.tool)
        || run.checkpoint.pending_tools.first() != Some(&p.tool)
        || cowork_domain::processes::process_request_hash(&run.record, &p.tool) != p.request_sha256
        || cowork_domain::processes::process_invocation(&p.tool)? != r.invocation
        || !programs.contains_key(&r.invocation.program)
    {
        return Err(Error::Forbidden);
    }
    let reference = run
        .record
        .spec
        .environment
        .as_ref()
        .ok_or(Error::Forbidden)?;
    let row =
        sqlx::query("SELECT revision,payload FROM environments WHERE project_id=$1 AND id=$2")
            .bind(&run.record.spec.project_id)
            .bind(&reference.environment_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    let environment =
        environments::checked(&row, &run.record.spec.project_id, &reference.environment_id)?;
    let pin = EnvironmentPin {
        environment_id: reference.environment_id.clone(),
        project_id: run.record.spec.project_id.clone(),
        version_id: reference.version_id.clone(),
        definition_sha256: reference.definition_sha256.clone(),
        qualification_sha256: reference.qualification_sha256.clone(),
        definition: environment
            .version(&reference.version_id)?
            .definition
            .clone(),
    };
    environment.resolve_pin(&pin, observation, now())?;
    // Use the same portable validator as the recipient, including the initial
    // launch lifetime. The placeholder manifest is only for this shape check.
    ManagedInputGrant {
        format_version: 1,
        execution_plan_id: plan.plan.id.clone(),
        execution_plan_sha256: plan.sha256.clone(),
        target: target.clone(),
        runner: runner.clone(),
        input_manifest_sha256: "0".repeat(64),
        process_attempt_id: p.id.clone(),
        run_id: r.run_id.clone(),
        request_sha256: r.request_sha256.clone(),
        environment: r.environment.clone(),
        invocation_sha256: canonical_hash(&r.invocation),
        worker_fence_sha256: r.worker_fence_sha256.clone(),
        lease_epoch: r.lease_epoch,
        issued_at: r.issued_at,
        expires_at: r.expires_at,
    }
    .validate_for_launch(&binding(runner), target, r, now())
    .map_err(|_| Error::Forbidden)?;
    Ok(ManagedInputContext {
        plan,
        run: run.record,
        workspace,
    })
}

fn validate_transfer(transfer: &ManagedInputTransfer, context: &ManagedInputContext) -> Result<()> {
    let ExecutionDestinationV2::ManagedRunner { target, runner, .. } = &context.plan.plan.target
    else {
        return Err(Error::Forbidden);
    };
    transfer
        .grant
        .verify("managed_input_grant", &runner.control_public_key)
        .map_err(|_| Error::Forbidden)?;
    let g = &transfer.grant.payload;
    g.validate_for_launch(&binding(runner), target, &transfer.request.payload, now())
        .map_err(|_| Error::Forbidden)?;
    cowork_domain::workspace_transfer::validate(&transfer.manifest)?;
    verify_workspace_snapshot(&transfer.snapshot)?;
    if g.execution_plan_id != context.plan.plan.id
        || g.execution_plan_sha256 != context.plan.sha256
        || g.input_manifest_sha256 != transfer.manifest.sha256
        || g.expires_at > transfer.request.payload.expires_at
        || transfer.snapshot.run_id != context.run.id
        || transfer.snapshot.order_id != context.run.spec.order_id
    {
        return Err(Error::ExecutionPlanChanged);
    }
    let aliases = &context.plan.workspace.copies;
    if transfer.snapshot.entries.iter().any(|entry| {
        !aliases
            .iter()
            .any(|copy| entry.path.split('/').next() == Some(copy.alias.as_str()))
    }) {
        return Err(Error::Forbidden);
    }
    let directories: Vec<_> = transfer
        .snapshot
        .entries
        .iter()
        .filter(|e| e.sha256.is_none())
        .map(|e| e.path.clone())
        .collect();
    let files: Vec<_> = transfer
        .snapshot
        .entries
        .iter()
        .filter(|e| e.sha256.is_some())
        .collect();
    if transfer.manifest.directories != directories
        || transfer.manifest.files.len() != files.len()
        || transfer
            .manifest
            .files
            .iter()
            .zip(files)
            .any(|(file, entry)| {
                file.path != entry.path
                    || file.size != entry.size
                    || Some(&file.sha256) != entry.sha256.as_ref()
            })
    {
        return Err(Error::ExecutionPlanChanged);
    }
    Ok(())
}

pub(super) fn checked(row: &sqlx::any::AnyRow) -> Result<ManagedInputTransfer> {
    let transfer: ManagedInputTransfer =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if canonical_hash(&transfer)
        != row
            .try_get::<String, _>("custody_sha256")
            .map_err(storage_error)?
        || canonical_hash(&transfer.grant)
            != row
                .try_get::<String, _>("grant_sha256")
                .map_err(storage_error)?
        || transfer.request.payload.process_attempt_id
            != row
                .try_get::<String, _>("attempt_id")
                .map_err(storage_error)?
        || transfer.request.payload.run_id
            != row.try_get::<String, _>("run_id").map_err(storage_error)?
        || transfer.grant.payload.execution_plan_id
            != row.try_get::<String, _>("plan_id").map_err(storage_error)?
    {
        return Err(Error::ExecutionPlanChanged);
    }
    Ok(transfer)
}

impl Store {
    pub async fn managed_input_context(
        &self,
        token: &str,
        request: &Signed<LaunchRequest>,
        observation: &EnvironmentObservation,
    ) -> Result<ManagedInputContext> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let _money = model_money::lock_creation(&mut tx).await?;
        let current = context(&mut tx, token, request, observation).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(current)
    }

    pub async fn retain_managed_input_transfer(
        &self,
        token: &str,
        transfer: &ManagedInputTransfer,
        observation: &EnvironmentObservation,
    ) -> Result<ManagedInputTransfer> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let _money = model_money::lock_creation(&mut tx).await?;
        let current = context(&mut tx, token, &transfer.request, observation).await?;
        validate_transfer(transfer, &current)?;
        sqlx::query("INSERT INTO owner_managed_input_transfers(attempt_id,run_id,plan_id,grant_sha256,custody_sha256,payload) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(attempt_id) DO NOTHING")
            .bind(&transfer.request.payload.process_attempt_id).bind(&current.run.id).bind(&current.plan.plan.id)
            .bind(canonical_hash(&transfer.grant)).bind(canonical_hash(transfer)).bind(json_string(transfer)?)
            .execute(&mut *tx).await.map_err(storage_error)?;
        let row = sqlx::query("SELECT * FROM owner_managed_input_transfers WHERE attempt_id=$1")
            .bind(&transfer.request.payload.process_attempt_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let original = checked(&row)?;
        if canonical_hash(&original) != canonical_hash(transfer) {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(original)
    }

    /// Fresh authority on every read; this never recreates ProcessLaunchPermit.
    pub async fn managed_input_transfer(
        &self,
        run: &str,
        token: &str,
        attempt: &str,
        observation: &EnvironmentObservation,
    ) -> Result<(ManagedInputTransfer, ManagedInputContext)> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let _money = model_money::lock_creation(&mut tx).await?;
        let row = sqlx::query(
            "SELECT * FROM owner_managed_input_transfers WHERE attempt_id=$1 AND run_id=$2",
        )
        .bind(attempt)
        .bind(run)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let transfer = checked(&row)?;
        let current = context(&mut tx, token, &transfer.request, observation).await?;
        validate_transfer(&transfer, &current)?;
        tx.commit().await.map_err(storage_error)?;
        Ok((transfer, current))
    }
}
