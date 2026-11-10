//! Built-in document and image tools. Inputs are SHA-checked copies, the engine runs in a
//! private toolbox stage, and results are published create-only through the file broker
//! with the same in-flight/reconcile protocol as other file operations.
use super::document_service::{failure, output_target, validate_document_scope};
use super::*;
use cowork_domain::documents::parse_document_call;

impl Runtime {
    /// A retained preview image, readable only through a run whose tool results reference it.
    pub async fn run_preview(&self, run_id: &str, sha256: &str) -> Result<Vec<u8>> {
        let stored = self.store.stored_run(run_id).await?;
        let referenced = stored.checkpoint.messages.iter().any(|message| {
            message["cowork_images"]
                .as_array()
                .is_some_and(|images| images.iter().any(|image| image["sha256"] == sha256))
        });
        let workflow_referenced = if !referenced && stored.record.spec.workflow.is_some() {
            let workflow = self
                .store
                .stored_workflow(&stored.record.spec.project_id, run_id)
                .await?;
            workflow
                .agents
                .values()
                .flat_map(|agent| &agent.turns)
                .flat_map(|turn| &turn.tool_results)
                .filter(|receipt| super::workflow_documents::document_tool(&receipt.tool.name))
                .any(|receipt| {
                    receipt.result["cowork_images"]
                        .as_array()
                        .is_some_and(|images| images.iter().any(|image| image["sha256"] == sha256))
                })
        } else {
            false
        };
        if !referenced && !workflow_referenced {
            return Err(Error::NotFound);
        }
        let files = self.files.clone().ok_or(Error::NotFound)?;
        let sha256 = sha256.to_owned();
        tokio::task::spawn_blocking(move || files.read_preview(&sha256))
            .await
            .map_err(storage_error)?
    }

    /// Document tools are granted automatically once the pinned engine is ready.
    pub(crate) fn document_tools_ready(&self) -> bool {
        self.toolbox
            .as_ref()
            .is_some_and(|t| t.availability().documents_available)
    }

    pub(crate) fn require_document_tools(&self) -> Result<()> {
        let available = self.toolbox.as_ref().map(|toolbox| toolbox.availability());
        if available
            .as_ref()
            .is_some_and(|value| value.document_status == "data_path_too_long")
        {
            return Err(Error::Validation(
                "workflow_document_runtime_path_too_long".into(),
            ));
        }
        if available.is_none_or(|value| !value.documents_available) {
            return Err(Error::Validation(
                "workflow_document_runtime_unavailable".into(),
            ));
        }
        Ok(())
    }

    /// Returns true after storing a tool error for a call that cannot succeed; such a
    /// call never reaches an approval. Already decided approvals are not rechecked here.
    pub(super) async fn reject_document_call(
        &self,
        run_id: &str,
        token: &str,
        stored: &StoredRun,
        tool: &ToolCall,
    ) -> Result<bool> {
        if stored
            .checkpoint
            .approvals
            .iter()
            .any(|a| a.tool == *tool && a.decision.is_some())
        {
            return Ok(false);
        }
        match self.document_precheck(&stored.record, tool).await {
            Ok(()) => Ok(false),
            Err(error) => {
                self.tool_result(run_id, token, tool, failure(&error), None)
                    .await?;
                Ok(true)
            }
        }
    }

    /// Only the original confirmed document destination can authorize COW document IO.
    async fn snapshot_document_revision(&self, spec: &RunSpec) -> Result<Option<String>> {
        if spec.input_workspace.is_none() {
            return Ok(None);
        }
        self.assert_snapshot_controller(spec).await?;
        let expected = self
            .store
            .pinned_document_engine_revision(spec)
            .await?
            .ok_or(Error::ExecutionPlanChanged)?;
        if self
            .toolbox
            .as_ref()
            .is_none_or(|toolbox| toolbox.document_engine_revision() != expected)
        {
            return Err(Error::ExecutionPlanChanged);
        }
        Ok(Some(expected))
    }

    /// Free, side-effect-free checks before an approval is requested, so a request that
    /// cannot succeed returns to the model instead of asking the user first.
    pub(super) async fn document_precheck(&self, run: &RunRecord, tool: &ToolCall) -> Result<()> {
        let call = parse_document_call(tool)?;
        validate_document_scope(run, &call)?;
        self.snapshot_document_revision(&run.spec).await?;
        let available = self
            .toolbox
            .as_ref()
            .ok_or(Error::Forbidden)?
            .availability();
        if available.document_status == "data_path_too_long"
            || call.operation == "compile_latex" && available.latex_status == "data_path_too_long"
        {
            return Err(Error::Validation(
                "workflow_document_runtime_path_too_long".into(),
            ));
        }
        if !available.documents_available {
            return Err(Error::Validation(
                "The document runtime is not installed yet".into(),
            ));
        }
        if call.operation == "ocr" && !available.ocr_available {
            return Err(Error::Validation(
                "workflow_document_ocr_unavailable".into(),
            ));
        }
        if call.operation == "compile_latex" && !available.latex_available {
            return Err(Error::Validation(
                "LaTeX is not installed yet; the tool setup has not finished".into(),
            ));
        }
        if call.operation == "convert" && !available.office_convert_available {
            return Err(Error::Validation(
                "Conversion needs LibreOffice, which is not installed".into(),
            ));
        }
        let output = output_target(run, &call)?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let run = run.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            if let Some((folder_id, path)) = output {
                let stat = files.stat(&run, &FileLocation { folder_id, path }).map_err(|error| match error {
                    Error::Forbidden => Error::Validation("The output folder is missing; create it with create_directory first".into()),
                    other => other,
                })?;
                if stat.node.is_some() {
                    return Err(Error::Validation(
                        "output.path already exists and is never overwritten; choose a new name such as report-v2.docx".into(),
                    ));
                }
            }
            for input in call.inputs.iter().filter(|i| i.expected_sha256.is_some()) {
                let stat = files.stat(&run, &FileLocation { folder_id: input.folder_id.clone(), path: input.path.clone() })?;
                if stat.node.map(|n| n.sha256) != input.expected_sha256 {
                    return Err(Error::FileConflict);
                }
            }
            Ok(())
        })
        .await
        .map_err(storage_error)?
    }

    pub(super) async fn invoke_document_tool(
        &self,
        run_id: &str,
        token: &str,
        stored: &StoredRun,
        tool: &ToolCall,
        stop: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        let expected = match self.snapshot_document_revision(&stored.record.spec).await {
            Ok(expected) => expected,
            Err(error) => {
                return self
                    .tool_result(run_id, token, tool, failure(&error), None)
                    .await;
            }
        };
        let execution = match self
            .prepare_document_execution(
                &stored.record,
                token,
                tool,
                stop,
                deadline,
                expected.as_deref(),
            )
            .await?
        {
            Ok(execution) => execution,
            Err(error) => {
                return self
                    .tool_result(run_id, token, tool, failure(&error), None)
                    .await;
            }
        };
        let result = execution.result;
        let images = execution.images;
        let Some(publication) = execution.publication else {
            return self
                .tool_result_with_images(run_id, token, tool, result, images)
                .await;
        };
        let super::document_service::DocumentPublication {
            folder_id,
            path,
            content,
            provenance,
        } = publication;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let run = stored.record.clone();
        let publishing = tool.clone();
        let task = tokio::task::spawn_blocking(move || {
            files.prepare_generated(&run, &publishing, &folder_id, &path, &content, provenance)
        });
        let prepared = match self
            .wait_file_task(run_id, token, stop, deadline, task)
            .await?
        {
            Ok(prepared) => prepared,
            Err(error) => {
                return self
                    .tool_result(run_id, token, tool, failure(&error), None)
                    .await;
            }
        };
        let operation = prepared.operation.clone();
        self.store.mutate(run_id, Some(token), |r| {
            ensure_running(r)?;
            ensure_pending(r, tool)?;
            if r.checkpoint.in_flight.is_some() {
                return Err(Error::UncertainEffect);
            }
            r.checkpoint.in_flight = Some(tool.clone());
            r.checkpoint.pending_file_operation = Some(operation.clone());
            Ok(Change::event(
                "file_operation_started",
                json!({"operation":file_operations::tool_summary(&operation),"fingerprint":approval_fingerprint(&r.record,tool)}),
            ))
        })
        .await?;
        let task = tokio::task::spawn_blocking(move || (prepared.apply(), prepared));
        let (applied, guard) = self
            .wait_file_task(run_id, token, stop, deadline, task)
            .await?;
        let applied = match applied {
            Ok(()) => true,
            Err(Error::FileConflict) => false,
            Err(_) => return Err(Error::UncertainEffect),
        };
        self.store
            .mutate(run_id, Some(token), |r| {
                ensure_running(r)?;
                ensure_pending(r, tool)?;
                if r.checkpoint.in_flight.as_ref() != Some(tool) {
                    return Err(Error::Conflict);
                }
                r.checkpoint.in_flight = None;
                r.checkpoint.pending_file_operation = None;
                r.checkpoint.pending_tools.remove(0);
                r.record.steps += 1;
                let content = if applied {
                    let mut summary = file_operations::tool_summary(&operation);
                    summary["engine"] = result.clone();
                    summary
                } else {
                    json!({"error":"file_conflict","message":"The output path was created meanwhile; choose a new name"})
                };
                let mut message = json!({"role":"tool","tool_call_id":tool.id,"content":content.to_string()});
                if applied && !images.is_empty() {
                    message["cowork_images"] = json!(images);
                }
                r.checkpoint.messages.push(message);
                let mut change = Change::event("tool_completed", json!({"tool_call_id":tool.id,"name":tool.name,"result":content,"images":if applied {images.clone()} else {Vec::new()}}));
                if applied {
                    change.file_operations.push(operation.clone());
                }
                Ok(change)
            })
            .await?;
        drop(guard);
        Ok(())
    }
}
