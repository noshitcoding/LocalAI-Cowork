use super::*;
use cowork_domain::{environments::*, processes::*};
use cowork_policy::{Decision, evaluate};

/// In-memory ownership, never serialized/reconstructed on an idempotent replay.
/// The qualified runner must still revalidate authority at its actual launch boundary.
pub struct ProcessLaunchPermit {
    attempt: ProcessAttempt,
    origin: Store,
}
impl ProcessLaunchPermit {
    pub fn attempt(&self) -> &ProcessAttempt {
        &self.attempt
    }
    pub fn into_attempt(self) -> ProcessAttempt {
        self.attempt
    }
    /// Consume the original, untransferred capability after local preparation
    /// failed. A stored attempt or replay can never manufacture this proof.
    pub async fn reject_before_transport(self, token: &str, code: &str) -> Result<ProcessAttempt> {
        valid_text(code, 128)?;
        let mut process = self
            .origin
            .stored_process(&self.attempt.run_id, &self.attempt.tool.id)
            .await?
            .ok_or(Error::NotFound)?;
        if canonical_hash(&process.record) != canonical_hash(&self.attempt)
            || process.record.state != ProcessAttemptState::LaunchCommitted
            || process.lease_sha256 != hash(token.as_bytes())
        {
            return Err(Error::Conflict);
        }
        let evidence_sha256 = canonical_hash(&json!({
            "kind":"controller_rejected_before_transport", "attempt":self.attempt,
            "lease_sha256":process.lease_sha256, "error_code":code
        }));
        process.record.state = ProcessAttemptState::ReconciledNotStarted { evidence_sha256 };
        process.record.revision = 2;
        process.record.updated_at = now();
        process.validate()?;
        let commit = self.origin.mutate(&process.record.run_id, Some(token), |r| {
            in_flight(r, &process.record)?;
            if !matches!(r.record.state, RunState::Running | RunState::CancelRequested) {
                return Err(Error::Conflict);
            }
            settle_checkpoint(r, &process.record.tool, json!({
                "not_started":true, "automatically_retried":false,
                "controller_before_transport":true, "error_code":code
            }));
            let mut change = Change::event("process_rejected_before_transport", json!({
                "process_attempt_id":process.record.id, "tool_call_id":process.record.tool.id,
                "not_started":true, "automatic_retry":false, "error_code":code
            }));
            if r.record.state == RunState::CancelRequested {
                r.transition(RunState::Canceled)?;
                r.lease = None;
                change.events.push(("run_canceled".into(), json!({"state":"canceled"})));
            }
            change.process_write = Some(ProcessWrite { stored:process.clone(), expected_revision:Some(1), owner_reconciliation:None });
            Ok(change)
        }).await;
        self.origin
            .process_commit_result(commit, process.record)
            .await
    }
}
pub enum ProcessAdmission {
    Launch(ProcessLaunchPermit),
    Existing(ProcessAttempt),
}

#[derive(Clone)]
pub struct ProcessWrite {
    pub(crate) stored: StoredProcess,
    pub(crate) expected_revision: Option<i64>,
    pub(super) owner_reconciliation: Option<managed_processes::OwnerReconciliationProof>,
}

#[derive(Clone)]
pub enum ProcessResolution {
    /// Only a trusted adapter/human review of retained evidence may conclude this.
    Completed {
        result: ProcessResult,
        evidence_sha256: String,
    },
    NotStarted {
        evidence_sha256: String,
    },
}

impl Store {
    async fn stored_process(&self, run: &str, call: &str) -> Result<Option<StoredProcess>> {
        let stored = self.stored_run(run).await?;
        let row = sqlx::query(
            "SELECT id,tool_call_id,revision,payload FROM process_attempts WHERE run_id=$1 AND tool_call_id=$2",
        )
        .bind(run)
        .bind(call)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        row.map(|row| {
            let p = checked_record(&row, run)?;
            validate_binding(&p.record, &stored.record)?;
            if p.record.tool.id != call {
                return Err(Error::Forbidden);
            }
            Ok(p)
        })
        .transpose()
    }
    pub async fn process_attempts(&self, run: &str) -> Result<Vec<ProcessAttempt>> {
        let stored = self.stored_run(run).await?;
        let rows = sqlx::query(
            "SELECT id,tool_call_id,revision,payload FROM process_attempts WHERE run_id=$1 ORDER BY id",
        )
        .bind(run)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        records(rows, &stored.record)
    }
    /// The only successful first admission returns a launch permit. Every replay is
    /// observation only, even if the original reply or OS start was never observed.
    pub async fn admit_process(
        &self,
        run: &str,
        token: &str,
        tool: &ToolCall,
        observation: &EnvironmentObservation,
    ) -> Result<ProcessAdmission> {
        let initial = self.stored_run(run).await?;
        initial.assert_lease(token, now())?;
        let request_sha256 = process_request_hash(&initial.record, tool);
        if let Some(existing) = self.stored_process(run, &tool.id).await? {
            return replay(existing, &request_sha256);
        }
        let invocation = process_invocation(tool)?;
        let workspace = initial
            .record
            .spec
            .folders
            .iter()
            .find(|g| {
                g.order_id.as_deref() == Some(initial.record.spec.order_id.as_str()) && g.writable
            })
            .ok_or(Error::Forbidden)?;
        let environment = initial
            .record
            .spec
            .environment
            .clone()
            .ok_or(Error::Forbidden)?;
        let timestamp = now();
        let stored = StoredProcess {
            lease_sha256: hash(token.as_bytes()),
            record: ProcessAttempt {
                id: id(),
                run_id: run.into(),
                project_id: initial.record.spec.project_id.clone(),
                tool: tool.clone(),
                environment,
                workspace_resource_id: workspace.resource_id.clone(),
                request_sha256: request_sha256.clone(),
                state: ProcessAttemptState::LaunchCommitted,
                revision: 1,
                created_at: timestamp,
                updated_at: timestamp,
            },
        };
        stored.validate()?;
        let result=self.mutate(run,Some(token),|r| {
            pending(r,tool)?;
            if r.record.state!=RunState::Running || r.checkpoint.in_flight.is_some() {return Err(Error::UncertainEffect);}
            if r.record.spec.environment!=Some(stored.record.environment.clone()) || process_request_hash(&r.record,tool)!=request_sha256 {return Err(Error::Conflict);}
            if evaluate(&r.record,tool,&[],&[],&r.checkpoint.approvals)!=Decision::Allow {return Err(Error::Forbidden);}
            if time_budget_exhausted(&r.record,now()) || r.record.steps>=r.record.spec.budgets.max_steps
                || invocation.timeout_seconds.saturating_mul(1000)>remaining_run_millis(&r.record,now()) {return Err(Error::BudgetExceeded);}
            r.checkpoint.in_flight=Some(tool.clone());
            let mut change=Change::event("process_launch_committed",json!({"process_attempt_id":stored.record.id,"tool_call_id":tool.id,"request_sha256":request_sha256,"environment":stored.record.environment}));
            change.claim_folders=true;
            change.environment_observation=Some(observation.clone());
            change.process_write=Some(ProcessWrite{stored:stored.clone(),expected_revision:None,owner_reconciliation:None});
            Ok(change)
        }).await;
        match result {
            Ok(_) => Ok(ProcessAdmission::Launch(ProcessLaunchPermit {
                attempt: stored.record,
                origin: self.clone(),
            })),
            Err(error) => {
                // A lost acknowledgement can leave the commit present. Never regenerate a permit.
                if let Some(existing) = self.stored_process(run, &tool.id).await? {
                    replay(existing, &request_sha256)
                } else {
                    Err(error)
                }
            }
        }
    }
    pub async fn finish_process(
        &self,
        run: &str,
        token: &str,
        call: &str,
        result: ProcessResult,
    ) -> Result<ProcessAttempt> {
        validate_process_result(&result)?;
        let mut process = self
            .stored_process(run, call)
            .await?
            .ok_or(Error::NotFound)?;
        let state = ProcessAttemptState::Completed {
            result: result.clone(),
        };
        if process.record.state != ProcessAttemptState::LaunchCommitted {
            return if process.record.state == state {
                Ok(process.record)
            } else {
                Err(Error::Conflict)
            };
        }
        if process.lease_sha256 != hash(token.as_bytes()) {
            return Err(Error::StaleLease);
        }
        process.record.state = state;
        process.record.revision = 2;
        process.record.updated_at = now();
        process.validate()?;
        let commit=self.mutate(run,Some(token),|r| {
            in_flight(r,&process.record)?;
            if !matches!(r.record.state,RunState::Running|RunState::CancelRequested) {return Err(Error::Conflict);}
            settle_checkpoint(r,&process.record.tool,process_result_summary(&result));
            let mut change=Change::event("process_completed",json!({"process_attempt_id":process.record.id,"tool_call_id":call,"termination":result.termination,"stdout_sha256":result.stdout.sha256,"stderr_sha256":result.stderr.sha256}));
            if r.record.state==RunState::CancelRequested {
                r.transition(RunState::Canceled)?;r.lease=None;
                change.events.push(("run_canceled".into(),json!({"state":"canceled"})));
            }
            change.process_write=Some(ProcessWrite{stored:process.clone(),expected_revision:Some(1),owner_reconciliation:None});
            Ok(change)
        }).await;
        self.process_commit_result(commit, process.record).await
    }
    /// Reconciliation is a separate trusted control-plane action, never an automatic retry.
    /// An evidence hash references retained reviewed proof; the hash does not authenticate it.
    pub async fn reconcile_process(
        &self,
        run: &str,
        call: &str,
        expected_request_sha256: &str,
        resolution: ProcessResolution,
    ) -> Result<ProcessAttempt> {
        let mut process = self
            .stored_process(run, call)
            .await?
            .ok_or(Error::NotFound)?;
        if process.record.request_sha256 != expected_request_sha256 {
            return Err(Error::Conflict);
        }
        let (state, summary) = match resolution {
            ProcessResolution::Completed {
                result,
                evidence_sha256,
            } => {
                validate_process_result(&result)?;
                let summary = process_result_summary(&result);
                (
                    ProcessAttemptState::ReconciledCompleted {
                        result,
                        evidence_sha256,
                    },
                    summary,
                )
            }
            ProcessResolution::NotStarted { evidence_sha256 } => (
                ProcessAttemptState::ReconciledNotStarted { evidence_sha256 },
                json!({"not_started":true,"automatically_retried":false}),
            ),
        };
        if process.record.state != ProcessAttemptState::LaunchCommitted {
            return if process.record.state == state {
                Ok(process.record)
            } else {
                Err(Error::Conflict)
            };
        }
        process.record.state = state;
        process.record.revision = 2;
        process.record.updated_at = now();
        process.validate()?;
        let commit=self.mutate(run,None,|r| {
            if r.record.state!=RunState::Interrupted || r.lease.is_some() {return Err(Error::Conflict);}
            in_flight(r,&process.record)?;
            settle_checkpoint(r,&process.record.tool,summary.clone());
            r.record.error_code=Some("executor_interrupted".into());
            let mut change=Change::event("process_reconciled",json!({"process_attempt_id":process.record.id,"tool_call_id":call,"automatic_retry":false}));
            change.process_write=Some(ProcessWrite{stored:process.clone(),expected_revision:Some(1),owner_reconciliation:None});
            Ok(change)
        }).await;
        self.process_commit_result(commit, process.record).await
    }
    async fn process_commit_result(
        &self,
        commit: Result<StoredRun>,
        proposed: ProcessAttempt,
    ) -> Result<ProcessAttempt> {
        match commit {
            Ok(_) => Ok(proposed),
            Err(error) => {
                if let Some(saved) = self
                    .stored_process(&proposed.run_id, &proposed.tool.id)
                    .await?
                    && saved.record.state == proposed.state
                {
                    return Ok(saved.record);
                }
                Err(error)
            }
        }
    }
}
fn replay(existing: StoredProcess, request: &str) -> Result<ProcessAdmission> {
    if existing.record.request_sha256 != request {
        Err(Error::Conflict)
    } else {
        Ok(ProcessAdmission::Existing(existing.record))
    }
}
fn pending(run: &StoredRun, tool: &ToolCall) -> Result<()> {
    if run.checkpoint.pending_tools.first() != Some(tool) {
        Err(Error::Conflict)
    } else {
        Ok(())
    }
}
pub(super) fn in_flight(run: &StoredRun, process: &ProcessAttempt) -> Result<()> {
    pending(run, &process.tool)?;
    if run.checkpoint.in_flight.as_ref() != Some(&process.tool)
        || process_request_hash(&run.record, &process.tool) != process.request_sha256
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
pub(super) fn settle_checkpoint(run: &mut StoredRun, tool: &ToolCall, summary: Value) {
    run.checkpoint.pending_tools.remove(0);
    run.checkpoint.in_flight = None;
    run.checkpoint
        .messages
        .push(json!({"role":"tool","tool_call_id":tool.id,"content":summary.to_string()}));
    run.record.steps += 1;
}
pub(super) fn checked_record(row: &sqlx::any::AnyRow, run: &str) -> Result<StoredProcess> {
    let process: StoredProcess =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    process.validate()?;
    let p = &process.record;
    if p.run_id != run
        || p.id != row.try_get::<String, _>("id").map_err(storage_error)?
        || p.tool.id
            != row
                .try_get::<String, _>("tool_call_id")
                .map_err(storage_error)?
        || p.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(process)
}
pub(super) fn records(
    rows: Vec<sqlx::any::AnyRow>,
    run: &RunRecord,
) -> Result<Vec<ProcessAttempt>> {
    rows.into_iter()
        .map(|row| {
            let p = checked_record(&row, &run.id)?.record;
            validate_binding(&p, run)?;
            Ok(p)
        })
        .collect()
}
fn validate_binding(p: &ProcessAttempt, run: &RunRecord) -> Result<()> {
    if p.project_id != run.spec.project_id
        || Some(&p.environment) != run.spec.environment.as_ref()
        || p.request_sha256 != process_request_hash(run, &p.tool)
        || !run.spec.folders.iter().any(|f| {
            f.order_id.as_deref() == Some(run.spec.order_id.as_str())
                && f.writable
                && f.resource_id == p.workspace_resource_id
        })
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
pub(super) async fn check_environment(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &RunRecord,
    observation: &EnvironmentObservation,
    write: &ProcessWrite,
) -> Result<()> {
    let binding = run.spec.environment.as_ref().ok_or(Error::Forbidden)?;
    if observation.runner_id != run.spec.executor || write.stored.record.environment != *binding {
        return Err(Error::Forbidden);
    }
    let changed =
        sqlx::query("UPDATE environments SET revision=revision WHERE project_id=$1 AND id=$2")
            .bind(&run.spec.project_id)
            .bind(&binding.environment_id)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?
            .rows_affected();
    if changed != 1 {
        return Err(Error::Forbidden);
    }
    let row =
        sqlx::query("SELECT revision,payload FROM environments WHERE project_id=$1 AND id=$2")
            .bind(&run.spec.project_id)
            .bind(&binding.environment_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    let environment =
        super::environments::checked(&row, &run.spec.project_id, &binding.environment_id)?;
    let version = environment.version(&binding.version_id)?;
    let pin = EnvironmentPin {
        environment_id: binding.environment_id.clone(),
        project_id: run.spec.project_id.clone(),
        version_id: binding.version_id.clone(),
        definition_sha256: binding.definition_sha256.clone(),
        qualification_sha256: binding.qualification_sha256.clone(),
        definition: version.definition.clone(),
    };
    environment.resolve_pin(&pin, observation, now())?;
    let invocation = process_invocation(&write.stored.record.tool)?;
    if !pin.definition.programs.contains_key(&invocation.program) {
        return Err(Error::Forbidden);
    }
    Ok(())
}
pub(super) async fn write_attempt(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &StoredRun,
    write: &ProcessWrite,
) -> Result<()> {
    let process = &write.stored;
    process.validate()?;
    if let Some(proof) = &write.owner_reconciliation {
        managed_processes::verify_write(tx, run, write, proof).await?;
    }
    let p = &process.record;
    if p.run_id != run.record.id
        || p.project_id != run.record.spec.project_id
        || run.record.spec.environment.as_ref() != Some(&p.environment)
        || p.request_sha256 != process_request_hash(&run.record, &p.tool)
        || !run.record.spec.folders.iter().any(|f| {
            f.order_id.as_deref() == Some(&run.record.spec.order_id)
                && f.writable
                && f.resource_id == p.workspace_resource_id
        })
    {
        return Err(Error::Forbidden);
    }
    let owner: Option<String> =
        sqlx::query_scalar("SELECT run_id FROM folder_owners WHERE resource_id=$1")
            .bind(&p.workspace_resource_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
    if owner.as_deref() != Some(&p.run_id) {
        return Err(Error::WorkspaceBusy);
    }
    let changed = match write.expected_revision {
        None => {
            if p.state != ProcessAttemptState::LaunchCommitted
                || run.checkpoint.in_flight.as_ref() != Some(&p.tool)
                || run
                    .lease
                    .as_ref()
                    .map(|l| hash(l.token.as_bytes()))
                    .as_ref()
                    != Some(&process.lease_sha256)
            {
                return Err(Error::Forbidden);
            }
            sqlx::query("INSERT INTO process_attempts(id,run_id,tool_call_id,revision,payload) VALUES($1,$2,$3,1,$4) ON CONFLICT(run_id,tool_call_id) DO NOTHING")
                .bind(&p.id).bind(&p.run_id).bind(&p.tool.id).bind(json_string(process)?).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()
        }
        Some(1) => {
            let payload: Option<String> = sqlx::query_scalar(
                "SELECT payload FROM process_attempts WHERE id=$1 AND run_id=$2 AND revision=1",
            )
            .bind(&p.id)
            .bind(&p.run_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
            let mut old: StoredProcess = parse(&payload.ok_or(Error::Conflict)?)?;
            old.validate()?;
            if old.record.state != ProcessAttemptState::LaunchCommitted
                || p.state == ProcessAttemptState::LaunchCommitted
                || run.checkpoint.in_flight.is_some()
            {
                return Err(Error::Conflict);
            }
            old.record.state = p.state.clone();
            old.record.updated_at = p.updated_at;
            old.record.revision = 2;
            if canonical_hash(&old) != canonical_hash(process) {
                return Err(Error::Forbidden);
            }
            sqlx::query("UPDATE process_attempts SET revision=2,payload=$1 WHERE id=$2 AND run_id=$3 AND revision=1")
                .bind(json_string(process)?).bind(&p.id).bind(&p.run_id).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()
        }
        _ => return Err(Error::Conflict),
    };
    if changed != 1 {
        return Err(Error::Conflict);
    }
    Ok(())
}
