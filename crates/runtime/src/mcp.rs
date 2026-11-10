use super::*;
use cowork_domain::mcp::mcp_binding;
use cowork_storage::McpAdmission;

impl Runtime {
    /// Trusted application entry point for an already connected, explicitly selected
    /// session. No endpoint/credential comes from model arguments. Product connection
    /// lifecycle/UI wiring is separate; this method never reconnects or replays.
    pub async fn dispatch_mcp_tool(
        &self,
        run: &str,
        token: &str,
        tool: &ToolCall,
        session: &mut cowork_mcp::Session,
        stop: &CancellationToken,
    ) -> Result<McpAttempt> {
        self.dispatch_mcp_inner(run, token, tool, session, stop, None)
            .await
    }
    pub(super) async fn dispatch_mcp_inner(
        &self,
        run: &str,
        token: &str,
        tool: &ToolCall,
        session: &mut cowork_mcp::Session,
        stop: &CancellationToken,
        cipher: Option<&cowork_mcp::SessionCipher>,
    ) -> Result<McpAttempt> {
        let stored = self.store.stored_run(run).await?;
        stored.assert_lease(token, now())?;
        // Observation must work even when the original session is poisoned/gone.
        if let Some(existing) = self
            .store
            .mcp_attempts(run)
            .await?
            .into_iter()
            .find(|a| a.tool.id == tool.id)
        {
            if existing.request_sha256 != cowork_domain::mcp::mcp_request_hash(&stored.record, tool)
            {
                return Err(Error::Conflict);
            }
            return Ok(existing);
        }
        if stop.is_cancelled() {
            return Err(Error::Provider("shutdown".into()));
        }
        let binding = mcp_binding(&stored.record, tool)?;
        let prepared = session
            .prepare(
                &binding.catalog_sha256,
                binding.definition["name"]
                    .as_str()
                    .ok_or(Error::Forbidden)?,
                tool.arguments.clone(),
            )
            .map_err(|_| {
                Error::Validation("MCP session or invocation is no longer valid".into())
            })?;
        // Acquire actual original-owner custody before an augmented invocation
        // can leave the process. Required tasks cannot become ordinary results.
        let task_keys = if prepared.task_ttl_ms().is_some() {
            Some(self.native_mcp_keys().await?)
        } else {
            None
        };
        let permit = match self.store.admit_mcp(run, token, tool, prepared).await? {
            McpAdmission::Existing(attempt) => return Ok(*attempt),
            McpAdmission::Dispatch(permit) => permit,
        };
        if self
            .store
            .validate_mcp_dispatch(run, token, permit.attempt())
            .await
            .is_err()
        {
            let _ = self
                .store
                .interrupt_mcp(run, token, &tool.id, "mcp_dispatch_rejected", None)
                .await;
            return Err(Error::UncertainEffect);
        }
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(remaining_run_millis(&stored.record, now()));
        let observed = {
            let call = session.call(permit.into_prepared());
            tokio::pin!(call);
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            let mut heartbeat = now();
            loop {
                tokio::select! {
                    biased;
                    _=stop.cancelled()=>break Err(("mcp_shutdown".to_string(),None)),
                    _=tokio::time::sleep_until(deadline)=>break Err(("mcp_budget_exhausted".to_string(),None)),
                    _=interval.tick()=>{
                        match self.store.stored_run(run).await {
                            Ok(current)=>{
                                if current.assert_lease(token,now()).is_err() {break Err(("mcp_lease_lost".into(),None));}
                                if current.record.state!=RunState::Running {break Err(("mcp_canceled".into(),None));}
                                if time_budget_exhausted(&current.record,now()) {break Err(("mcp_budget_exhausted".into(),None));}
                                if now()-heartbeat>=3000 {
                                    if self.store.heartbeat(run,token).await.is_err() {break Err(("mcp_lease_lost".into(),None));}
                                    heartbeat=now();
                                }
                            },
                            Err(_)=>break Err(("mcp_storage_unavailable".into(),None)),
                        }
                    },
                    outcome=&mut call=>break match outcome {
                        Ok(result)=>Ok(result),
                        Err(error)=>Err((error.code.to_string(),error.response().cloned())),
                    },
                }
            }
        };
        let observed = match observed {
            Ok(result) if result.task.is_some() => match task_keys.as_ref() {
                // Keep the optional task state out of ordinary MCP dispatch frames.
                Some((keys, key)) => Box::pin(self.await_native_mcp_task(
                    run,
                    token,
                    tool,
                    session,
                    result,
                    cipher,
                    keys.as_ref(),
                    *key,
                    stop,
                ))
                .await
                .map_err(|e| (e.code().to_owned(), None)),
                None => Err(("mcp_task_custody_unavailable".into(), None)),
            },
            outcome => outcome,
        };
        match observed {
            Ok(result) => {
                let result = McpResult {
                    response: result.response,
                    sha256: result.response_sha256,
                };
                let finished = if let Some(cipher) = cipher {
                    match self
                        .mcp_session_completion(run, tool, session, cipher)
                        .await
                    {
                        Ok(completion) => {
                            self.store
                                .finish_mcp_session(
                                    run,
                                    token,
                                    &tool.id,
                                    result.clone(),
                                    completion,
                                )
                                .await
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    self.store
                        .finish_mcp(run, token, &tool.id, result.clone())
                        .await
                };
                match finished {
                    Ok(attempt) => Ok(attempt),
                    Err(_) => {
                        let _ = self
                            .store
                            .interrupt_mcp(
                                run,
                                token,
                                &tool.id,
                                "mcp_receipt_unconfirmed",
                                Some(result.response),
                            )
                            .await;
                        Err(Error::UncertainEffect)
                    }
                }
            }
            Err((code, response)) => {
                // If the lease/database was lost, the original committed marker is
                // still authoritative; normal lease recovery will interrupt the run.
                let _ = self
                    .store
                    .interrupt_mcp(run, token, &tool.id, &code, response)
                    .await;
                Err(Error::UncertainEffect)
            }
        }
    }
}
