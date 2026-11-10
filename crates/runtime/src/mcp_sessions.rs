use super::*;
use cowork_domain::mcp::mcp_binding;
use cowork_mcp::{Endpoint, Limits, Session, SessionCipher};
use cowork_storage::{McpConnectionAdmission, McpConnectionState, McpSessionCompletion};

/// Explicit trusted-owner inputs. Credential lookup/key custody is separate from
/// this service; neither key nor bearer is accepted through public run contracts.
pub struct McpSessionAccess<'a> {
    pub cipher: &'a SessionCipher,
    pub bearer: Option<&'a str>,
    pub limits: Limits,
}
/// Trusted native/server owner, never deserialized from model or browser input.
pub struct McpSecretAccess {
    pub keys: Arc<dyn cowork_secrets::KeyStore>,
    pub active_key: cowork_secrets::KeyId,
    pub limits: Limits,
}
static KEY_RESOLUTION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
impl Runtime {
    /// Resolve the exact encrypted credential and the original checkpoint key.
    /// Missing/locked keys cannot generate replacements or anonymous connections.
    pub async fn dispatch_custodied_mcp_tool(
        &self,
        run: &str,
        token: &str,
        tool: &ToolCall,
        access: &McpSecretAccess,
        stop: &CancellationToken,
    ) -> Result<McpAttempt> {
        let stored = self.store.stored_run(run).await?;
        stored.assert_lease(token, now())?;
        if let Some(attempt) = self
            .store
            .mcp_attempts(run)
            .await?
            .into_iter()
            .find(|a| a.tool.id == tool.id)
        {
            if attempt.request_sha256 != cowork_domain::mcp::mcp_request_hash(&stored.record, tool)
            {
                return Err(Error::Conflict);
            }
            return Ok(attempt);
        }
        if stop.is_cancelled() {
            return Err(Error::McpSessionUnavailable);
        }
        if stored.record.state != RunState::Running
            || stored.checkpoint.in_flight.is_some()
            || stored.checkpoint.pending_tools.first() != Some(tool)
        {
            return Err(Error::Conflict);
        }
        if evaluate(&stored.record, tool, &[], &[], &stored.checkpoint.approvals) != Decision::Allow
        {
            return Err(Error::Forbidden);
        }
        let binding = mcp_binding(&stored.record, tool)?;
        let server = self
            .store
            .mcp_server(&stored.record.spec.project_id, &binding.server_id)
            .await?;
        if !server.bindings()?.contains(binding) {
            return Err(Error::Forbidden);
        }
        let oauth_bearer = if let Some(reference) = server
            .configuration
            .credential_reference
            .as_deref()
            .filter(|v| v.starts_with("oauth:"))
        {
            Some(
                self.native_mcp_oauth_bearer(
                    &stored.record.spec.project_id,
                    &server.id,
                    &server.configuration.endpoint,
                    reference,
                    server.configuration.credential_revision,
                )
                .await?,
            )
        } else {
            None
        };
        let credential = self.store.mcp_server_credential(&server).await?;
        let key = match self.store.mcp_connection(run, &binding.server_id).await? {
            Some(connection) => {
                if connection.state != McpConnectionState::Ready {
                    return Err(Error::McpSessionUnavailable);
                }
                cowork_secrets::KeyId::from_cipher_id(
                    connection
                        .sealed
                        .as_ref()
                        .ok_or(Error::McpSessionUnavailable)?
                        .key_id(),
                )
                .map_err(|_| Error::McpSessionUnavailable)?
            }
            None => access.active_key,
        };
        // Native DBus/Win32 calls do not occupy a Tokio executor thread. Timed-out
        // calls retain their slot until they actually return; no unbounded backlog.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let slot = tokio::select! {
            biased;
            _=stop.cancelled()=>return Err(Error::McpSessionUnavailable),
            result=tokio::time::timeout_at(deadline,KEY_RESOLUTION.acquire())=>result
                .map_err(|_|Error::McpSessionUnavailable)?.map_err(|_|Error::McpSessionUnavailable)?,
        };
        let keys = access.keys.clone();
        let load = tokio::task::spawn_blocking(move || {
            let _slot = slot;
            let material = keys.load(&key).map_err(|_| Error::McpSessionUnavailable)?;
            let cipher = SessionCipher::new(
                &key.cipher_id().map_err(|_| Error::McpSessionUnavailable)?,
                material,
            )
            .map_err(|_| Error::McpSessionUnavailable)?;
            let bearer = credential
                .map(|record| {
                    cowork_secrets::open_bearer(
                        keys.as_ref(),
                        &record.scope,
                        record.sealed.as_ref().ok_or(Error::McpSessionUnavailable)?,
                    )
                    .map_err(|_| Error::McpSessionUnavailable)
                })
                .transpose()?
                .or(oauth_bearer);
            Ok::<_, Error>((cipher, bearer))
        });
        let (cipher, bearer) = tokio::select! {
            biased;
            _=stop.cancelled()=>return Err(Error::McpSessionUnavailable),
            result=tokio::time::timeout_at(deadline,load)=>result
                .map_err(|_|Error::McpSessionUnavailable)?
                .map_err(|_|Error::McpSessionUnavailable)??,
        };
        self.dispatch_persistent_mcp_tool(
            run,
            token,
            tool,
            &McpSessionAccess {
                cipher: &cipher,
                bearer: bearer.as_ref().map(|v| v.as_str()),
                limits: access.limits,
            },
            stop,
        )
        .await
    }
    /// Continue the original remote session from a current durable encrypted
    /// checkpoint. Never initialize a replacement or replay an admitted tool.
    pub async fn dispatch_persistent_mcp_tool(
        &self,
        run: &str,
        token: &str,
        tool: &ToolCall,
        access: &McpSessionAccess<'_>,
        stop: &CancellationToken,
    ) -> Result<McpAttempt> {
        let stored = self.store.stored_run(run).await?;
        stored.assert_lease(token, now())?;
        if let Some(attempt) = self
            .store
            .mcp_attempts(run)
            .await?
            .into_iter()
            .find(|a| a.tool.id == tool.id)
        {
            if attempt.request_sha256 != cowork_domain::mcp::mcp_request_hash(&stored.record, tool)
            {
                return Err(Error::Conflict);
            }
            return Ok(attempt);
        }
        if stop.is_cancelled() {
            return Err(Error::McpSessionUnavailable);
        }
        if stored.record.state != RunState::Running
            || stored.checkpoint.in_flight.is_some()
            || stored.checkpoint.pending_tools.first() != Some(tool)
        {
            return Err(Error::Conflict);
        }
        if evaluate(&stored.record, tool, &[], &[], &stored.checkpoint.approvals) != Decision::Allow
        {
            return Err(Error::Forbidden);
        }
        let binding = mcp_binding(&stored.record, tool)?;
        let server = self
            .store
            .mcp_server(&stored.record.spec.project_id, &binding.server_id)
            .await?;
        if !server.bindings()?.contains(binding)
            || server.configuration.credential_reference.is_some() != access.bearer.is_some()
        {
            return Err(Error::Forbidden);
        }
        let endpoint = Endpoint::new(&binding.endpoint, access.bearer)
            .map_err(|_| Error::McpSessionUnavailable)?;
        // The connection permit can hold the sole SQLite connection until the
        // handshake is persisted. Read UI lifecycle options before taking it.
        let lifecycle = self
            .store
            .library_preferences(cowork_storage::library_config::LibraryOwner::Local)
            .await?
            .value;
        let mut session = match self
            .store
            .begin_mcp_connection(run, token, &binding.server_id)
            .await?
        {
            McpConnectionAdmission::Existing(connection) => {
                if connection.state != McpConnectionState::Ready {
                    return Err(Error::McpSessionUnavailable);
                }
                let scope = connection.scope(
                    &stored.record,
                    connection
                        .checkpoint_revision
                        .ok_or(Error::McpSessionUnavailable)?,
                )?;
                let session = access
                    .cipher
                    .open(
                        connection
                            .sealed
                            .as_ref()
                            .ok_or(Error::McpSessionUnavailable)?,
                        &scope,
                        endpoint,
                        access.limits,
                        server.configuration.catalog,
                    )
                    .map_err(|_| Error::McpSessionUnavailable)?;
                if connection.connection_id.as_ref() != Some(&session.connection_id())
                    || connection.epoch != Some(session.epoch())
                {
                    return Err(Error::McpSessionUnavailable);
                }
                session
            }
            McpConnectionAdmission::Connect(permit) => {
                let connect = self.connect_owner_mcp_catalog_with_preferences(
                    &binding.server_id,
                    &binding.endpoint,
                    access.bearer,
                    access.limits,
                    Some(&binding.catalog_sha256),
                    true,
                    &lifecycle,
                );
                let session = self.await_mcp_connection(run, token, stop, connect).await?;
                let scope = permit.record().scope(&stored.record, 2)?;
                let sealed = access
                    .cipher
                    .seal(&session, &scope)
                    .map_err(|_| Error::McpSessionUnavailable)?;
                self.store
                    .finish_mcp_connection(
                        run,
                        token,
                        permit,
                        session.connection_id(),
                        session.epoch(),
                        sealed,
                    )
                    .await?;
                session
            }
        };
        self.dispatch_mcp_inner(run, token, tool, &mut session, stop, Some(access.cipher))
            .await
    }
    async fn await_mcp_connection<F, T>(
        &self,
        run: &str,
        token: &str,
        stop: &CancellationToken,
        future: F,
    ) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        let stored = self.store.stored_run(run).await?;
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(remaining_run_millis(&stored.record, now()).min(120_000));
        tokio::pin!(future);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        let mut heartbeat = now();
        loop {
            tokio::select! {
                biased;
                _=stop.cancelled()=>return Err(Error::McpSessionUnavailable),
                _=tokio::time::sleep_until(deadline)=>return Err(Error::BudgetExceeded),
                _=interval.tick()=>{
                    let current=self.store.stored_run(run).await?;
                    current.assert_lease(token,now())?;
                    if current.record.state!=RunState::Running {return Err(Error::McpSessionUnavailable);}
                    if time_budget_exhausted(&current.record,now()) {return Err(Error::BudgetExceeded);}
                    if now()-heartbeat>=3000 {self.store.heartbeat(run,token).await?;heartbeat=now();}
                },
                result=&mut future=>return result,
            }
        }
    }
    pub(super) async fn mcp_session_completion(
        &self,
        run: &str,
        tool: &ToolCall,
        session: &Session,
        cipher: &SessionCipher,
    ) -> Result<McpSessionCompletion> {
        let stored = self.store.stored_run(run).await?;
        let binding = mcp_binding(&stored.record, tool)?;
        let connection = self
            .store
            .mcp_connection(run, &binding.server_id)
            .await?
            .ok_or(Error::McpSessionUnavailable)?;
        let scope = connection.scope(&stored.record, connection.next_revision()?)?;
        match cipher.seal(session, &scope) {
            Ok(sealed) => Ok(McpSessionCompletion::Ready {
                record_id: connection.id,
                expected_revision: connection.revision,
                connection_id: session.connection_id(),
                epoch: session.epoch(),
                sealed,
            }),
            Err(cowork_mcp::Error::CatalogChanged) => Ok(McpSessionCompletion::Unavailable {
                code: "mcp_catalog_changed".into(),
            }),
            Err(_) => Err(Error::McpSessionUnavailable),
        }
    }
}
