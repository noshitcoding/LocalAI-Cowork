//! Scoped workflow IO. The proposal and dispatch are journaled before any effect.
use super::*;
use cowork_storage::{WorkflowToolExecution, workflow_toolbox::NativeWorkflowAdmission};

enum FileGuard {
    Write(Box<cowork_broker::PreparedWrite>),
    Operation(Box<cowork_broker::PreparedOperation>),
}
pub(super) fn external(name: &str) -> bool {
    name.starts_with("mcp_")
        || matches!(
            name,
            "list_files"
                | "read_text_file"
                | "stat_file"
                | "write_text_file"
                | "create_directory"
                | "rename_file"
                | "remove_file"
                | "restore_file"
        )
}
impl Runtime {
    pub(crate) async fn invoke_workflow_external(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn: &str,
        raw: &str,
        stop: &CancellationToken,
    ) -> Result<WorkflowToolExecution> {
        let (record, tool) = match self
            .store
            .admit_workflow_toolbox(project, run, lease, turn, raw)
            .await?
        {
            NativeWorkflowAdmission::Dispatch { record, tool } => (*record, tool),
            NativeWorkflowAdmission::Completed(r) => {
                return Ok(WorkflowToolExecution::Completed(r));
            }
            NativeWorkflowAdmission::AwaitingApproval(a) => {
                return Ok(WorkflowToolExecution::AwaitingApproval(a));
            }
        };
        self.store
            .assert_workflow_toolbox_dispatch(project, run, lease, turn, raw)
            .await?;
        self.assert_snapshot_controller(&record.spec).await?;
        if stop.is_cancelled() {
            return Err(Error::UncertainEffect);
        }
        let (value, guard) = if tool.name.starts_with("mcp_") {
            (
                self.workflow_mcp_call(&record, &tool, lease, turn, raw, stop)
                    .await?,
                None,
            )
        } else if tool.name == "execute_program" {
            (
                self.invoke_workflow_program(&record, &tool, lease, turn, raw, stop)
                    .await?,
                None,
            )
        } else {
            let files = self.files.as_ref().ok_or(Error::Forbidden)?.clone();
            files.validate_spec_grants(&record.spec)?;
            let retained = if tool.name == "restore_file" {
                let restore: RestoreFile =
                    serde_json::from_value(tool.arguments.clone()).map_err(storage_error)?;
                Some(self.retained_content(&record.spec, &restore).await?)
            } else {
                None
            };
            let file_stop = stop.clone();
            let store = self.store.clone();
            let source_project = project.to_owned();
            let source_run = run.to_owned();
            let source_turn = turn.to_owned();
            let source_raw = raw.to_owned();
            let source_lease = lease.to_owned();
            let handle = tokio::runtime::Handle::current();
            if !matches!(
                tool.name.as_str(),
                "write_text_file"
                    | "create_directory"
                    | "rename_file"
                    | "remove_file"
                    | "restore_file"
            ) {
                store
                    .retain_workflow_external_preparation(
                        project,
                        run,
                        turn,
                        raw,
                        "file_read",
                        json!({"read_only":true}),
                    )
                    .await?;
            }
            let task = tokio::task::spawn_blocking(
                move || -> Result<(Value, Option<FileGuard>)> {
                    let (project, run, turn, raw) =
                        (&source_project, &source_run, &source_turn, &source_raw);
                    if file_stop.is_cancelled() {
                        return Err(Error::UncertainEffect);
                    }
                    let location = || {
                        serde_json::from_value::<FileLocation>(tool.arguments.clone())
                            .map_err(storage_error)
                    };
                    let mut result = match tool.name.as_str() {
                        "list_files" => serde_json::to_value(files.list(&record, &location()?)?)
                            .map_err(storage_error)?,
                        "read_text_file" => {
                            serde_json::to_value(files.read(&record, &location()?)?)
                                .map_err(storage_error)?
                        }
                        "stat_file" => serde_json::to_value(files.stat(&record, &location()?)?)
                            .map_err(storage_error)?,
                        "write_text_file" => {
                            let mut prepared = files.prepare_write(&record, &tool)?;
                            handle.block_on(store.retain_workflow_external_preparation(
                                project,
                                run,
                                turn,
                                raw,
                                "file_write",
                                json!({"change":prepared.change}),
                            ))?;
                            if file_stop.is_cancelled() {
                                return Err(Error::UncertainEffect);
                            }
                            handle.block_on(store.assert_workflow_toolbox_dispatch(
                                project,
                                run,
                                &source_lease,
                                turn,
                                raw,
                            ))?;
                            prepared.apply()?;
                            let value = json!({"change":prepared.change,"_workflow_success":true});
                            handle.block_on(store.retain_workflow_external_outcome(
                                project,
                                run,
                                &tool.id,
                                value.clone(),
                            ))?;
                            return Ok((value, Some(FileGuard::Write(Box::new(prepared)))));
                        }
                        "create_directory" | "rename_file" | "remove_file" | "restore_file" => {
                            let prepared = if let Some(content) = retained {
                                files.prepare_restore(&record, &tool, &content)?
                            } else {
                                files.prepare_operation(&record, &tool)?
                            };
                            handle.block_on(store.retain_workflow_external_preparation(
                                project,
                                run,
                                turn,
                                raw,
                                "file_operation",
                                json!({"operation_receipt":prepared.operation}),
                            ))?;
                            if file_stop.is_cancelled() {
                                return Err(Error::UncertainEffect);
                            }
                            handle.block_on(store.assert_workflow_toolbox_dispatch(
                                project,
                                run,
                                &source_lease,
                                turn,
                                raw,
                            ))?;
                            prepared.apply()?;
                            let value = json!({"operation_receipt":prepared.operation,"_workflow_success":true});
                            handle.block_on(store.retain_workflow_external_outcome(
                                project,
                                run,
                                &tool.id,
                                value.clone(),
                            ))?;
                            return Ok((value, Some(FileGuard::Operation(Box::new(prepared)))));
                        }
                        _ => return Err(Error::Forbidden),
                    };
                    result["_workflow_success"] = json!(true);
                    handle.block_on(store.retain_workflow_external_outcome(
                        project,
                        run,
                        &tool.id,
                        result.clone(),
                    ))?;
                    Ok((result, None))
                },
            );
            let parent = self.store.stored_run(run).await?;
            let deadline = tokio::time::Instant::now()
                + Duration::from_millis(remaining_run_millis(&parent.record, now()));
            self.wait_file_task(run, lease, stop, deadline, task)
                .await??
        };
        let result = self
            .store
            .finish_workflow_toolbox(project, run, lease, turn, raw, value)
            .await;
        // Keep the OS capability lock until the durable receipt is committed.
        if let Some(guard) = guard {
            match guard {
                FileGuard::Write(value) => drop(value),
                FileGuard::Operation(value) => drop(value),
            }
        }
        result.map(WorkflowToolExecution::Completed)
    }
    async fn workflow_mcp_call(
        &self,
        record: &RunRecord,
        tool: &ToolCall,
        expected_lease: &str,
        turn: &str,
        raw: &str,
        stop: &CancellationToken,
    ) -> Result<Value> {
        let binding = cowork_domain::mcp::mcp_binding(record, tool)?;
        let server = self
            .store
            .mcp_server(&record.spec.project_id, &binding.server_id)
            .await?;
        if !server.bindings()?.contains(binding) {
            return Err(Error::Forbidden);
        }
        let bearer = self
            .workflow_mcp_bearer(&record.spec.project_id, binding)
            .await?;
        let endpoint =
            cowork_mcp::Endpoint::new(&binding.endpoint, bearer.as_deref().map(String::as_str))
                .map_err(|_| Error::McpSessionUnavailable)?;
        let owned_runtime = self.clone();
        let owned_record = record.clone();
        let owned_tool = tool.clone();
        let binding = binding.clone();
        let owned_lease = expected_lease.to_owned();
        let turn = turn.to_owned();
        let raw = raw.to_owned();
        let owned_stop = stop.clone();
        // The bounded source call owns receipt capture independently of the
        // awaiting workflow future. Losing the lease does not resend this RPC.
        let operation = tokio::spawn(async move {
            let record = &owned_record;
            let tool = &owned_tool;
            let runtime = &owned_runtime;
            let mut session =
                cowork_mcp::Session::connect_interactive(endpoint, cowork_mcp::Limits::default())
                    .await
                    .map_err(|_| Error::McpSessionUnavailable)?;
            let catalog = session
                .discover()
                .await
                .map_err(|_| Error::McpSessionUnavailable)?;
            if catalog.sha256() != binding.catalog_sha256 {
                return Err(Error::Conflict);
            }
            let name = binding.definition["name"]
                .as_str()
                .ok_or(Error::Forbidden)?;
            let prepared = session
                .prepare(&binding.catalog_sha256, name, tool.arguments.clone())
                .map_err(|_| Error::Forbidden)?;
            let task_keys = if prepared.task_ttl_ms().is_some() {
                Some(runtime.native_mcp_keys().await?)
            } else {
                None
            };
            runtime.store.retain_workflow_external_preparation(&record.spec.project_id,&record.id,&turn,&raw,"mcp",json!({"binding":binding,"connection_id":prepared.connection_id(),"connection_epoch":prepared.epoch(),"rpc_request_id":prepared.rpc_request_id(),"request_sha256":prepared.request_sha256(),"original_request_sha256":canonical_hash(&json!(["workflow-mcp-request/1",record.spec,tool,binding])),"catalog_sha256":prepared.catalog_sha256(),"protocol":prepared.protocol(),"task_ttl_ms":prepared.task_ttl_ms()})).await?;
            let task_source = if let Some((_, key)) = &task_keys {
                Some(
                    runtime
                        .store
                        .workflow_mcp_task_source(
                            &record.spec.project_id,
                            &record.id,
                            &owned_lease,
                            &turn,
                            &raw,
                            *key,
                        )
                        .await?,
                )
            } else {
                None
            };
            let current = runtime
                .store
                .mcp_server(&record.spec.project_id, &binding.server_id)
                .await?;
            if !current.bindings()?.contains(&binding) {
                return Err(Error::Forbidden);
            }
            let parent = runtime.store.stored_run(&record.id).await?;
            parent.assert_lease(&owned_lease, now())?;
            if owned_stop.is_cancelled()
                || parent.record.state != RunState::Running
                || time_budget_exhausted(&parent.record, now())
            {
                return Err(Error::UncertainEffect);
            }
            let result = session
                .call(prepared)
                .await
                .map_err(|_| Error::UncertainEffect)?;
            let result = if result.task.is_some() {
                let (keys, key) = task_keys.ok_or(Error::Forbidden)?;
                runtime
                    .await_workflow_mcp_task(
                        &record.spec.project_id,
                        &record.id,
                        &owned_lease,
                        &turn,
                        &raw,
                        &mut session,
                        result,
                        task_source.ok_or(Error::Forbidden)?,
                        keys.as_ref(),
                        key,
                        &owned_stop,
                    )
                    .await?
            } else {
                result
            };
            let value = json!({"response":result.response,"response_sha256":result.response_sha256,"_workflow_success":true});
            runtime
                .store
                .retain_workflow_external_outcome(
                    &record.spec.project_id,
                    &record.id,
                    &tool.id,
                    value.clone(),
                )
                .await?;
            Ok::<_, Error>(value)
        });
        tokio::pin!(operation);
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let mut heartbeat = tokio::time::Instant::now();
        loop {
            tokio::select! {
                biased;
                _=stop.cancelled()=>return Err(Error::UncertainEffect),
                result=&mut operation=>return result.map_err(storage_error)?,
                _=tick.tick()=>{
                    let parent=self.store.stored_run(&record.id).await?;
                    let lease=parent.lease.as_ref().ok_or(Error::StaleLease)?;
                    parent.assert_lease(expected_lease,now())?;
                    if parent.record.state!=RunState::Running||time_budget_exhausted(&parent.record,now()){return Err(Error::UncertainEffect);}
                    if heartbeat.elapsed()>=Duration::from_secs(3){self.store.heartbeat(&record.id,&lease.token).await?;heartbeat=tokio::time::Instant::now();}
                }
            }
        }
    }
}
