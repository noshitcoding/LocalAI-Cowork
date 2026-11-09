use super::*;
use cowork_broker::RetainedContent;

impl Runtime {
    /// Restore uses immutable receipt references, never an arbitrary private blob hash.
    pub(super) async fn retained_content(
        &self,
        spec: &RunSpec,
        input: &RestoreFile,
    ) -> Result<RetainedContent> {
        let copy = spec.input_workspace.is_some() && input.expected_source_sha256.is_some();
        self.retained_restore_content(spec, input, copy).await
    }

    /// Preparation resolves a readable receipt before a new copy exists. This
    /// returns retained metadata only; no IO permit or workload is admitted.
    pub(super) async fn retained_restore_content(
        &self,
        spec: &RunSpec,
        input: &RestoreFile,
        copy: bool,
    ) -> Result<RetainedContent> {
        let source = self.store.stored_run(&input.source.run_id).await?.record;
        if source.spec.order_id != spec.order_id || source.spec.project_id != spec.project_id {
            return Err(Error::Forbidden);
        }
        let target = spec
            .folders
            .iter()
            .find(|f| f.id == input.folder_id && f.writable)
            .ok_or(Error::Forbidden)?;
        let original = source
            .spec
            .folders
            .iter()
            .find(|f| f.id == input.folder_id)
            .ok_or(Error::Forbidden)?;
        if (!copy && original.resource_id != target.resource_id)
            || original.order_id != target.order_id
        {
            return Err(Error::Forbidden);
        }
        let call = &input.source.tool_call_id;
        let content: Result<RetainedContent> = match input.source.version {
            RetainedVersion::TextBefore | RetainedVersion::TextAfter => {
                let receipt = self
                    .store
                    .file_changes(&source.id)
                    .await?
                    .into_iter()
                    .find(|c| &c.tool_call_id == call)
                    .ok_or(Error::NotFound)?;
                if receipt.folder_id != input.folder_id {
                    return Err(Error::Forbidden);
                }
                let sha256 = if input.source.version == RetainedVersion::TextBefore {
                    receipt.before_sha256.ok_or(Error::NotFound)?
                } else {
                    receipt.after_sha256
                };
                Ok(RetainedContent::File { sha256 })
            }
            RetainedVersion::OperationBefore | RetainedVersion::OperationAfter => {
                let receipt = self
                    .store
                    .file_operations(&source.id)
                    .await?
                    .into_iter()
                    .find(|c| &c.tool_call_id == call)
                    .ok_or(Error::NotFound)?;
                if receipt.folder_id != input.folder_id {
                    return Err(Error::Forbidden);
                }
                let node = if input.source.version == RetainedVersion::OperationBefore {
                    receipt.before
                } else {
                    receipt.after
                }
                .ok_or(Error::NotFound)?;
                Ok(match node.kind {
                    FileNodeKind::File => RetainedContent::File {
                        sha256: node.sha256,
                    },
                    FileNodeKind::Directory => RetainedContent::Directory {
                        sha256: node.sha256,
                        entries: receipt.entries,
                    },
                })
            }
        };
        let content = content?;
        let sha = match &content {
            RetainedContent::File { sha256 } | RetainedContent::Directory { sha256, .. } => sha256,
        };
        if input
            .expected_source_sha256
            .as_ref()
            .is_some_and(|expected| expected != sha)
        {
            return Err(Error::Conflict);
        }
        Ok(content)
    }

    pub async fn create_restore_run(&self, input: &CreateRestoreRun) -> Result<RunRecord> {
        if let Some(existing) = self.store.existing_restore_request(input).await? {
            return Ok(existing);
        }
        let created = self.prepare_restore_run(input).await;
        if matches!(created, Err(Error::Conflict))
            && let Some(existing) = self.store.existing_restore_request(input).await?
        {
            return Ok(existing);
        }
        created
    }

    async fn prepare_restore_run(&self, input: &CreateRestoreRun) -> Result<RunRecord> {
        let spec = self.restore_source_spec(input).await?;
        self.retained_content(&spec, &input.restore).await?;
        self.store.create_restore_run(input, spec).await
    }

    pub(super) async fn restore_source_spec(&self, input: &CreateRestoreRun) -> Result<RunSpec> {
        valid_text(&input.prompt, 64_000)?;
        valid_text(&input.idempotency_key, 128)?;
        if input.access_mode == AccessMode::ReadOnly {
            return Err(Error::Forbidden);
        }
        let order = self.store.order(&input.order_id).await?;
        if order.revision != input.base_revision {
            return Err(Error::Conflict);
        }
        let source = self
            .store
            .stored_run(&input.restore.source.run_id)
            .await?
            .record;
        if source.spec.order_id != order.id || source.spec.project_id != order.project_id {
            return Err(Error::Forbidden);
        }
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let mut folders = vec![self.workspace(&order.id).await?.folder];
        if folders[0].id != input.restore.folder_id {
            folders.push(
                files
                    .grants(&order.project_id)?
                    .into_iter()
                    .find(|f| f.id == input.restore.folder_id && f.writable)
                    .ok_or(Error::Forbidden)?,
            );
        }
        let spec = RunSpec {
            input_workspace: None,
            agent_config: None,
            model_binding: None,
            output: None,
            workflow: None,
            mcp_tools: vec![],
            environment: None,
            order_id: order.id,
            project_id: order.project_id,
            prompt: input.prompt.clone(),
            provider: None,
            file_action: Some(ToolCall {
                id: "restore".into(),
                name: "restore_file".into(),
                arguments: json!(input.restore),
            }),
            access_mode: input.access_mode.clone(),
            executor: "local-v2".into(),
            policy_revision: 1,
            allowed_tools: vec!["restore_file".into()],
            budgets: Budgets::default(),
            folders,
        };
        // Resolve provenance before accepting work. Actual bytes, target and current grants
        // are checked again under the broker lock at execution time.
        Ok(spec)
    }

    pub(super) async fn finish_file_action(
        &self,
        run_id: &str,
        token: &str,
        stored: &StoredRun,
    ) -> Result<bool> {
        let tool = stored
            .record
            .spec
            .file_action
            .as_ref()
            .ok_or(Error::Forbidden)?;
        let message = stored
            .checkpoint
            .messages
            .iter()
            .rev()
            .find(|m| m["role"] == "tool" && m["tool_call_id"] == tool.id)
            .ok_or(Error::Conflict)?;
        let result: Value =
            serde_json::from_str(message["content"].as_str().ok_or(Error::Conflict)?)
                .map_err(storage_error)?;
        let failure = result.get("error").and_then(Value::as_str);
        let kind = match tool.name.as_str() {
            "restore_file" => Some(FileOperationKind::Restore),
            "import_file" => Some(FileOperationKind::Import),
            "document_write" | "image_edit" => Some(FileOperationKind::Generate),
            "document_read" => None,
            _ => return Err(Error::Forbidden),
        };
        if failure.is_none()
            && kind.is_some()
            && !self
                .store
                .file_operations(run_id)
                .await?
                .iter()
                .any(|r| r.tool_call_id == tool.id && Some(r.kind.clone()) == kind)
        {
            return Err(Error::Conflict);
        }
        self.store
            .mutate(run_id, Some(token), |r| {
                ensure_running(r)?;
                if r.checkpoint.in_flight.is_some() || !r.checkpoint.pending_tools.is_empty() {
                    return Err(Error::Conflict);
                }
                if let Some(code) = failure {
                    r.transition(RunState::Failed)?;
                    r.lease = None;
                    r.record.error_code = Some(code.into());
                    Ok(Change::event(
                        "file_action_failed",
                        json!({"state":"failed","code":code}),
                    ))
                } else {
                    r.checkpoint.last_model_content = format!("✓ {}", r.record.spec.prompt);
                    r.checkpoint.completion_pending = true;
                    r.transition(RunState::WaitingForSnapshot)?;
                    Ok(Change::event(
                        "workspace_snapshot_pending",
                        json!({"state":"waiting_for_snapshot","source":"file_action"}),
                    ))
                }
            })
            .await?;
        Ok(failure.is_some())
    }
}
