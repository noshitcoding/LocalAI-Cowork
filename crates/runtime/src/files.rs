use super::*;

impl Runtime {
    pub(super) async fn invoke_file_tool(
        &self,
        run_id: &str,
        token: &str,
        stored: &StoredRun,
        tool: &ToolCall,
        stop: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        if matches!(
            tool.name.as_str(),
            "create_directory" | "rename_file" | "remove_file" | "restore_file" | "import_file"
        ) {
            return self
                .invoke_file_operation(run_id, token, stored, tool, stop, deadline)
                .await;
        }
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let record = stored.record.clone();
        let call = tool.clone();
        if tool.name != "write_text_file" {
            let task = tokio::task::spawn_blocking(move || {
                let location: FileLocation = serde_json::from_value(call.arguments)
                    .map_err(|_| Error::Validation("Invalid file location".into()))?;
                if call.name == "list_files" {
                    serde_json::to_value(files.list(&record, &location)?).map_err(storage_error)
                } else if call.name == "stat_file" {
                    serde_json::to_value(files.stat(&record, &location)?).map_err(storage_error)
                } else {
                    serde_json::to_value(files.read(&record, &location)?).map_err(storage_error)
                }
            });
            let result = self
                .wait_file_task(run_id, token, stop, deadline, task)
                .await?;
            let value = match result {
                Ok(value) => value,
                Err(error) => json!({"error":error.code(),"message":error.to_string()}),
            };
            return self.tool_result(run_id, token, tool, value, None).await;
        }
        let task = tokio::task::spawn_blocking(move || files.prepare_write(&record, &call));
        let prepared = self
            .wait_file_task(run_id, token, stop, deadline, task)
            .await?;
        let mut prepared = match prepared {
            Ok(p) => p,
            Err(error) => {
                return self
                    .tool_result(
                        run_id,
                        token,
                        tool,
                        json!({"error":error.code(),"message":error.to_string()}),
                        None,
                    )
                    .await;
            }
        };
        let change = prepared.change.clone();
        self.store
            .mutate(run_id, Some(token), |r| {
                ensure_running(r)?;
                ensure_pending(r, tool)?;
                if r.checkpoint.in_flight.is_some() {
                    return Err(Error::UncertainEffect);
                }
                r.checkpoint.in_flight = Some(tool.clone());
                r.checkpoint.pending_file_change = Some(change.clone());
                Ok(Change::event(
                    "file_write_started",
                    json!({"change":change,"fingerprint":approval_fingerprint(&r.record,tool)}),
                ))
            })
            .await?;
        // No optimistic-transaction closure executes a filesystem mutation.
        let task = tokio::task::spawn_blocking(move || {
            let outcome = prepared.apply();
            (outcome, prepared)
        });
        let (outcome, guard) = self
            .wait_file_task(run_id, token, stop, deadline, task)
            .await?;
        match outcome {
            Ok(()) => {
                self.complete_file_tool(
                    run_id,
                    token,
                    tool,
                    Some(&change),
                    json!({"change":change}),
                )
                .await?
            }
            Err(Error::FileConflict) => {
                self.complete_file_tool(run_id, token, tool, None, json!({"error":"file_conflict"}))
                    .await?
            }
            Err(_) => return Err(Error::UncertainEffect),
        }
        drop(guard);
        Ok(())
    }

    async fn complete_file_tool(
        &self,
        run_id: &str,
        token: &str,
        tool: &ToolCall,
        change: Option<&FileChange>,
        result: Value,
    ) -> Result<()> {
        self.store
            .mutate(run_id, Some(token), |r| {
                ensure_running(r)?;
                ensure_pending(r, tool)?;
                if r.checkpoint.in_flight.as_ref() != Some(tool) {
                    return Err(Error::Conflict);
                }
                r.checkpoint.pending_tools.remove(0);
                r.checkpoint.in_flight = None;
                r.checkpoint.pending_file_change = None;
                r.checkpoint.messages.push(
                    json!({"role":"tool","tool_call_id":tool.id,"content":result.to_string()}),
                );
                r.record.steps += 1;
                let mut outcome = Change::event(
                    "tool_completed",
                    json!({"tool_call_id":tool.id,"name":tool.name,"result":result}),
                );
                if let Some(change) = change {
                    outcome.file_changes.push(change.clone());
                }
                Ok(outcome)
            })
            .await?;
        Ok(())
    }

    pub async fn reconcile_file(&self, run_id: &str, input: ReconcileFile) -> Result<RunRecord> {
        let stored = self.store.stored_run(run_id).await?;
        if stored.record.state != RunState::Interrupted || stored.lease.is_some() {
            return Err(Error::Conflict);
        }
        if stored.checkpoint.pending_file_operation.is_some() {
            return self.reconcile_file_operation(run_id, input, &stored).await;
        }
        let tool = stored.checkpoint.in_flight.clone().ok_or(Error::Conflict)?;
        if tool.name != "write_text_file"
            || approval_fingerprint(&stored.record, &tool) != input.fingerprint
        {
            return Err(Error::Conflict);
        }
        let pending = stored
            .checkpoint
            .pending_file_change
            .clone()
            .ok_or(Error::Conflict)?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let run = stored.record.clone();
        self.restore_run_workspace(&run).await?;
        let change = pending.clone();
        let (observed, guard) =
            tokio::task::spawn_blocking(move || files.observe_change(&run, &change))
                .await
                .map_err(storage_error)??;
        let accepted = matches!(input.decision, ReconcileFileDecision::AcceptCompleted);
        if (accepted && observed.as_deref() != Some(pending.after_sha256.as_str()))
            || (!accepted && observed != pending.before_sha256)
        {
            return Err(Error::FileConflict);
        }
        let result=self.store.mutate(run_id,None,|r| {
            if r.record.state!=RunState::Interrupted || r.lease.is_some() || r.checkpoint.in_flight.as_ref()!=Some(&tool)
                || r.record.revision!=stored.record.revision {return Err(Error::Conflict);}
            ensure_pending(r,&tool)?;
            r.checkpoint.in_flight=None;r.checkpoint.pending_file_change=None;
            r.record.error_code=Some("executor_interrupted".into());
            let mut change=Change::event("file_write_reconciled",json!({"tool_call_id":tool.id,"accepted_completed":accepted,"observed_sha256":observed}));
            if accepted {
                r.checkpoint.pending_tools.remove(0);r.record.steps+=1;
                r.checkpoint.messages.push(json!({"role":"tool","tool_call_id":tool.id,"content":json!({"change":pending,"reconciled":true}).to_string()}));
                change.file_changes.push(pending.clone());
            }
            // Stay interrupted; reconciliation does not authorize the rest of the run.
            Ok(change)
        }).await?;
        drop(guard);
        Ok(result.record)
    }

    pub async fn previous_file_version(
        &self,
        run_id: &str,
        tool_call_id: &str,
    ) -> Result<TextFile> {
        self.file_version(run_id, tool_call_id, true).await
    }

    pub async fn written_file_version(&self, run_id: &str, tool_call_id: &str) -> Result<TextFile> {
        self.file_version(run_id, tool_call_id, false).await
    }

    async fn file_version(
        &self,
        run_id: &str,
        tool_call_id: &str,
        previous: bool,
    ) -> Result<TextFile> {
        let detail = self.store.detail(run_id).await?;
        let mut changes = detail.file_changes;
        changes.extend(detail.uncertain_file_change);
        let change = changes
            .into_iter()
            .find(|c| c.tool_call_id == tool_call_id)
            .ok_or(Error::NotFound)?;
        let sha = if previous {
            change.before_sha256.ok_or(Error::NotFound)?
        } else {
            change.after_sha256
        };
        let files = self.files.clone().ok_or(Error::NotFound)?;
        let wanted = sha.clone();
        let content = tokio::task::spawn_blocking(move || files.read_version(&wanted))
            .await
            .map_err(storage_error)??;
        Ok(TextFile {
            folder_id: change.folder_id,
            path: change.path,
            size: content.len() as u64,
            content,
            sha256: sha,
        })
    }
}
