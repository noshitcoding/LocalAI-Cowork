use super::*;

pub(super) fn tool_summary(operation: &FileOperation) -> Value {
    json!({"kind":operation.kind,"path":operation.path,"destination":operation.destination,
        "before":operation.before,"after":operation.after,"retained_entries":operation.entries.len()})
}

impl Runtime {
    // Blocking native IO cannot be assumed canceled when its await is dropped. The
    // in-flight marker and OS guard continue to fence writes until reconciliation.
    pub(super) async fn wait_file_task<T: Send + 'static>(
        &self,
        run_id: &str,
        token: &str,
        stop: &CancellationToken,
        deadline: tokio::time::Instant,
        mut task: tokio::task::JoinHandle<T>,
    ) -> Result<T> {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        let mut last_heartbeat = now();
        loop {
            tokio::select! {
                biased;
                _=stop.cancelled()=>return Err(Error::Provider("shutdown".into())),
                _=tokio::time::sleep_until(deadline)=>return Err(Error::BudgetExceeded),
                _=interval.tick()=>{
                    let current=self.store.stored_run(run_id).await?;current.assert_lease(token,now())?;
                    if current.record.state==RunState::CancelRequested {return Err(Error::Provider("canceled".into()));}
                    if time_budget_exhausted(&current.record,now()) {return Err(Error::BudgetExceeded);}
                    if now()-last_heartbeat>=3000 {self.store.heartbeat(run_id,token).await?;last_heartbeat=now();}
                },
                result=&mut task=>return result.map_err(storage_error),
            }
        }
    }

    pub(super) async fn invoke_file_operation(
        &self,
        run_id: &str,
        token: &str,
        stored: &StoredRun,
        tool: &ToolCall,
        stop: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let run = stored.record.clone();
        let call = tool.clone();
        let retained = if tool.name == "restore_file" {
            let input: RestoreFile =
                serde_json::from_value(tool.arguments.clone()).map_err(storage_error)?;
            match self.retained_content(&run.spec, &input).await {
                Ok(content) => Some(content),
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
            }
        } else {
            None
        };
        let upload = if tool.name == "import_file" {
            let input: ImportFile =
                serde_json::from_value(tool.arguments.clone()).map_err(storage_error)?;
            match self.import_source(&run.spec, &input).await {
                Ok(upload) => Some(upload),
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
            }
        } else {
            None
        };
        let task = tokio::task::spawn_blocking(move || {
            if let Some(upload) = upload {
                files.prepare_import(&run, &call, &upload)
            } else {
                match retained {
                    Some(content) => files.prepare_restore(&run, &call, &content),
                    None => files.prepare_operation(&run, &call),
                }
            }
        });
        let prepared = self
            .wait_file_task(run_id, token, stop, deadline, task)
            .await?;
        let prepared = match prepared {
            Ok(prepared) => prepared,
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
        let operation = prepared.operation.clone();
        self.store.mutate(run_id,Some(token),|r| {
            ensure_running(r)?;ensure_pending(r,tool)?;
            if r.checkpoint.in_flight.is_some() {return Err(Error::UncertainEffect);}
            r.checkpoint.in_flight=Some(tool.clone());r.checkpoint.pending_file_operation=Some(operation.clone());
            Ok(Change::event("file_operation_started",json!({"operation":tool_summary(&operation),"fingerprint":approval_fingerprint(&r.record,tool)})))
        }).await?;
        let task = tokio::task::spawn_blocking(move || (prepared.apply(), prepared));
        let (result, guard) = self
            .wait_file_task(run_id, token, stop, deadline, task)
            .await?;
        let applied = match result {
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
                let result = if applied {
                    tool_summary(&operation)
                } else {
                    json!({"error":"file_conflict"})
                };
                r.checkpoint.messages.push(
                    json!({"role":"tool","tool_call_id":tool.id,"content":result.to_string()}),
                );
                let mut change = Change::event(
                    "tool_completed",
                    json!({"tool_call_id":tool.id,"name":tool.name,"result":result}),
                );
                if applied {
                    change.file_operations.push(operation.clone());
                }
                Ok(change)
            })
            .await?;
        drop(guard);
        Ok(())
    }

    pub(super) async fn reconcile_file_operation(
        &self,
        run_id: &str,
        input: ReconcileFile,
        stored: &StoredRun,
    ) -> Result<RunRecord> {
        let tool = stored.checkpoint.in_flight.clone().ok_or(Error::Conflict)?;
        if !matches!(
            tool.name.as_str(),
            "create_directory"
                | "rename_file"
                | "remove_file"
                | "restore_file"
                | "import_file"
                | "document_write"
                | "image_edit"
        ) || approval_fingerprint(&stored.record, &tool) != input.fingerprint
        {
            return Err(Error::Conflict);
        }
        self.restore_run_workspace(&stored.record).await?;
        let operation = stored
            .checkpoint
            .pending_file_operation
            .clone()
            .ok_or(Error::Conflict)?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let run = stored.record.clone();
        let observed = operation.clone();
        let applied = matches!(input.decision, ReconcileFileDecision::AcceptCompleted);
        let guard = tokio::task::spawn_blocking(move || {
            files.reconcile_operation(&run, &observed, applied)
        })
        .await
        .map_err(storage_error)??;
        let result=self.store.mutate(run_id,None,|r| {
            if r.record.state!=RunState::Interrupted||r.lease.is_some()||r.record.revision!=stored.record.revision
                ||r.checkpoint.in_flight.as_ref()!=Some(&tool) {return Err(Error::Conflict);}
            ensure_pending(r,&tool)?;r.checkpoint.in_flight=None;r.checkpoint.pending_file_operation=None;
            r.record.error_code=Some("executor_interrupted".into());
            let mut change=Change::event("file_operation_reconciled",json!({"tool_call_id":tool.id,"accepted_completed":applied}));
            if applied {
                r.checkpoint.pending_tools.remove(0);r.record.steps+=1;
                r.checkpoint.messages.push(json!({"role":"tool","tool_call_id":tool.id,"content":tool_summary(&operation).to_string()}));
                change.file_operations.push(operation.clone());
            }
            Ok(change)
        }).await?;
        drop(guard);
        Ok(result.record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn slow_native_io_renews_lease_and_stays_in_flight_after_cancellation() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Native IO fixture".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id,
                title: "Slow IO".into(),
            })
            .await
            .unwrap();
        let profile: ProviderProfile = serde_json::from_value(json!({
            "id":"test","name":"Fixture","kind":"test","endpoint":"","model":"test","revision":1,
            "context_tokens":32768,"credential_required":false,"credential_ready":true
        }))
        .unwrap();
        let runtime = Runtime::new(
            store.clone(),
            vec![ConfiguredProvider {
                profile,
                credential_env: None,
            }],
            true,
        )
        .unwrap();
        let input = CreateRun {
            agent_config: None,
            model_binding: None,
            output: None,
            native_tools: vec![],
            environment: None,
            mcp_tools: vec![],
            budgets: cowork_contracts::Budgets::default(),
            order_id: order.id,
            base_revision: 1,
            idempotency_key: id(),
            prompt: "Slow native IO".into(),
            provider_id: "test".into(),
            provider_revision: 1,
            access_mode: AccessMode::ConfirmMutations,
            folders: vec![],
        };
        let run = runtime.create_run(&input).await.unwrap();
        let claim = store.claim().await.unwrap().unwrap();
        let lease = claim.lease.unwrap();
        store
            .mutate(&run.id, Some(&lease.token), |r| {
                assert_eq!(r.record.state, RunState::Running);
                r.checkpoint.in_flight = Some(ToolCall {
                    id: "native".into(),
                    name: "rename_file".into(),
                    arguments: json!({}),
                });
                Ok(Change::event("native_started", json!({})))
            })
            .await
            .unwrap();
        let (release, receiver) = std::sync::mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let observed = finished.clone();
        let native = tokio::task::spawn_blocking(move || {
            let _ = receiver.recv_timeout(Duration::from_secs(10));
            observed.store(true, Ordering::SeqCst);
        });
        let worker = runtime.clone();
        let run_id = run.id.clone();
        let token = lease.token.clone();
        let waiter = tokio::spawn(async move {
            worker
                .wait_file_task(
                    &run_id,
                    &token,
                    &CancellationToken::new(),
                    tokio::time::Instant::now() + Duration::from_secs(9),
                    native,
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(3400)).await;
        assert!(
            store
                .stored_run(&run.id)
                .await
                .unwrap()
                .lease
                .unwrap()
                .until
                > lease.until
        );
        store.cancel(&run.id).await.unwrap();
        assert!(matches!(waiter.await.unwrap(), Err(Error::Provider(_))));
        assert!(!finished.load(Ordering::SeqCst));
        assert!(
            store
                .stored_run(&run.id)
                .await
                .unwrap()
                .checkpoint
                .in_flight
                .is_some()
        );
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !finished.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        store.close().await;
    }
}
