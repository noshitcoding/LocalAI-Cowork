//! Quiescent reconciliation of retained source evidence. No new model/tool send.
use super::*;
use cowork_domain::workflows::{WorkflowOperation, WorkflowOutcome};
use cowork_storage::WorkflowExternalEvidence;
enum Guard {
    Write(cowork_broker::ResourceLock),
    Operation(cowork_broker::ResourceLock),
}
impl Runtime {
    pub async fn reconcile_workflow_effects(&self, project: &str, run: &str) -> Result<Value> {
        let parent = self.store.stored_run(run).await?;
        if parent.record.spec.project_id != project {
            return Err(Error::NotFound);
        }
        if parent.record.state != RunState::Interrupted || parent.lease.is_some() {
            return Err(Error::Conflict);
        }
        self.restore_run_workspace(&parent.record).await?;
        let workflow = self.store.stored_workflow(project, run).await?;
        let pending: Vec<_> = workflow
            .progress
            .attempts()
            .values()
            .filter(|a| {
                matches!(a.operation, WorkflowOperation::Tool { .. })
                    && (a.outcome.is_none() || a.outcome == Some(WorkflowOutcome::Uncertain))
            })
            .map(|a| a.id.clone())
            .collect();
        let mut settled = vec![];
        let mut unresolved = vec![];
        for call in pending {
            let Some(evidence) = self
                .store
                .workflow_external_evidence(project, run, &call)
                .await?
            else {
                unresolved.push(json!({"id":call,"reason":"source_preparation_unavailable"}));
                continue;
            };
            let result = self
                .observe_workflow_effect(&parent.record, &workflow, &evidence)
                .await;
            let (value, guard) = match result {
                Ok(result) => result,
                Err(error) => {
                    unresolved.push(json!({"id":call,"reason":error.code()}));
                    continue;
                }
            };
            // Source row is immutable once completed and survives loss of the
            // controller. The OS file lock is held until the graph receipt commits.
            self.store
                .retain_workflow_external_outcome(project, run, &call, value)
                .await?;
            let recorded = self
                .store
                .reconcile_workflow_external_outcome(project, run, &call)
                .await;
            if let Some(guard) = guard {
                match guard {
                    Guard::Write(g) | Guard::Operation(g) => drop(g),
                }
            }
            recorded?;
            settled.push(call);
        }
        Ok(
            json!({"run":self.store.stored_run(run).await?.record,"settled_calls":settled,"unresolved_calls":unresolved,"automatic_retry":false}),
        )
    }
    async fn observe_workflow_effect(
        &self,
        parent: &RunRecord,
        workflow: &cowork_storage::StoredWorkflow,
        evidence: &WorkflowExternalEvidence,
    ) -> Result<(Value, Option<Guard>)> {
        if let Some(value) = &evidence.result {
            return Ok((value.clone(), None));
        }
        let mut scoped = parent.clone();
        scoped.spec = self
            .store
            .workflow_node_spec(
                &parent.spec.project_id,
                &parent.id,
                &workflow.progress.attempts()[&evidence.tool.id]
                    .claim
                    .node_id,
            )
            .await?;
        match evidence.kind.as_str() {
            "file_write" => {
                let files = self.files.clone().ok_or(Error::Forbidden)?;
                let change: FileChange =
                    serde_json::from_value(evidence.preparation["change"].clone())
                        .map_err(storage_error)?;
                if change.tool_call_id != evidence.tool.id {
                    return Err(Error::Forbidden);
                }
                let observed = change.clone();
                let (value, guard) =
                    tokio::task::spawn_blocking(move || files.observe_change(&scoped, &observed))
                        .await
                        .map_err(storage_error)??;
                let result = if value.as_deref() == Some(change.after_sha256.as_str()) {
                    json!({"change":change,"_workflow_success":true,"reconciled":true})
                } else if value == change.before_sha256 {
                    json!({"_workflow_success":false,"reconciled_not_applied":true,"observed_sha256":value})
                } else {
                    return Err(Error::FileConflict);
                };
                Ok((result, Some(Guard::Write(guard))))
            }
            "file_operation" => {
                let files = self.files.clone().ok_or(Error::Forbidden)?;
                let operation: FileOperation =
                    serde_json::from_value(evidence.preparation["operation_receipt"].clone())
                        .map_err(storage_error)?;
                if operation.tool_call_id != evidence.tool.id {
                    return Err(Error::Forbidden);
                }
                let observed = operation.clone();
                let result = tokio::task::spawn_blocking(move || {
                    match files.reconcile_operation(&scoped, &observed, true) {
                        Ok(lock) => Ok((true, lock)),
                        Err(Error::FileConflict) => files
                            .reconcile_operation(&scoped, &observed, false)
                            .map(|lock| (false, lock)),
                        Err(error) => Err(error),
                    }
                })
                .await
                .map_err(storage_error)??;
                let value = if result.0 {
                    let mut value = json!({"operation_receipt":operation,"_workflow_success":true,"reconciled":true});
                    if workflow_documents::document_tool(&evidence.tool.name) {
                        let document = evidence.preparation["document"]
                            .as_object()
                            .ok_or(Error::Forbidden)?;
                        for (key, field) in document {
                            if !matches!(
                                key.as_str(),
                                "engine" | "cowork_images" | "document_engine_revision"
                            ) {
                                return Err(Error::Forbidden);
                            }
                            value[key] = field.clone();
                        }
                    }
                    value
                } else {
                    json!({"_workflow_success":false,"reconciled_not_applied":true})
                };
                Ok((value, Some(Guard::Operation(result.1))))
            }
            "file_read" if workflow_documents::document_tool(&evidence.tool.name) => {
                let mut value = evidence
                    .preparation
                    .get("document")
                    .filter(|value| value.is_object())
                    .cloned()
                    .ok_or(Error::UncertainEffect)?;
                if value
                    .get("_workflow_success")
                    .and_then(Value::as_bool)
                    .is_none()
                {
                    return Err(Error::UncertainEffect);
                }
                value["reconciled"] = json!(true);
                Ok((value, None))
            }
            "process_runner" => Ok((self.retained_workflow_runner_outcome(evidence).await?, None)),
            "native_toolbox" => {
                let service = self.toolbox.as_ref().ok_or(Error::Forbidden)?;
                let request: cowork_toolbox::Request =
                    serde_json::from_value(evidence.preparation["request"].clone())
                        .map_err(storage_error)?;
                let jobs = service
                    .jobs(&parent.spec.order_id)
                    .map_err(|_| Error::UncertainEffect)?;
                let job = jobs
                    .iter()
                    .find(|j| j.idempotency_key == request.idempotency_key)
                    .ok_or(Error::UncertainEffect)?;
                if job.request_sha256
                    != evidence.preparation["request_sha256"]
                        .as_str()
                        .ok_or(Error::Forbidden)?
                    || job.state != "completed"
                {
                    return Err(Error::UncertainEffect);
                }
                let outcome = job.outcome.as_ref().ok_or(Error::UncertainEffect)?;
                let mut value = toolbox_bridge::job_result(job);
                value["_workflow_success"] = json!(
                    outcome.termination == cowork_executor::Termination::Exited
                        && outcome.exit_code == Some(0)
                        && outcome.output_complete
                );
                value["reconciled"] = json!(true);
                Ok((value, None))
            }
            // A remote MCP effect cannot be inferred from a timeout or a human
            // checkbox. Only a retained exact source reply can settle it.
            _ => Err(Error::UncertainEffect),
        }
    }
}
