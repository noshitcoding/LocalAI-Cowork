//! Device-owned tools and credentials for signed offline delegation.
use super::*;
use cowork_secrets::{CredentialScope, SealedCredential, open_bearer, seal_bearer};
use cowork_storage::offline_execution::{OfflineMcpConnection, OfflineToolConfiguration};
use cowork_toolbox::{Action, Request, Toolbox};
use zeroize::Zeroizing;

fn bad() -> Error {
    Error::Validation("offline_tool_configuration_invalid".into())
}
fn scope(g: &OfflineGrant, reference: &str, endpoint: &str) -> CredentialScope {
    CredentialScope {
        project_id: g.project_id.to_string(),
        server_id: "offline-device-tools/v1".into(),
        reference: format!("{}/{}", g.id, reference),
        revision: 1,
        endpoint: endpoint.into(),
    }
}
pub(super) fn configured_tools(
    g: &OfflineGrant,
    c: Option<&OfflineToolConfiguration>,
) -> Vec<String> {
    g.allowed_tools
        .iter()
        .filter(|name| {
            matches!(
                name.as_str(),
                "list_sources" | "read_source" | "create_artifact"
            ) || c.is_some_and(|c| c.allowed_tools.contains(name))
        })
        .cloned()
        .collect()
}
impl Runtime {
    async fn offline_seal_tool_secret(
        &self,
        g: &OfflineGrant,
        reference: &str,
        endpoint: &str,
        secret: String,
    ) -> Result<SealedCredential> {
        let (device, _) = self.offline_signer().await?;
        if g.device != device.device {
            return Err(Error::Forbidden);
        }
        let manager = self.offline_keys(&device).await?;
        seal_bearer(
            manager.as_ref(),
            device.key_id,
            &scope(g, reference, endpoint),
            &Zeroizing::new(secret),
        )
        .map_err(|_| Error::NativeKeyUnavailable)
    }
    pub(super) async fn offline_open_tool_secret(
        &self,
        g: &OfflineGrant,
        reference: &str,
        endpoint: &str,
        sealed: &SealedCredential,
    ) -> Result<Zeroizing<String>> {
        let (device, _) = self.offline_signer().await?;
        if g.device != device.device {
            return Err(Error::Forbidden);
        }
        let manager = self.offline_keys(&device).await?;
        open_bearer(manager.as_ref(), &scope(g, reference, endpoint), sealed)
            .map_err(|_| Error::NativeKeyUnavailable)
    }
    pub async fn offline_configure_tools(
        &self,
        input: ConfigureOfflineTools,
    ) -> Result<OfflineToolConfigurationView> {
        let ticket = self.store.offline_local_ticket(input.grant_id).await?;
        cowork_storage::offline_execution::validate_offline_ticket(&ticket)?;
        let g = &ticket.grant;
        let available = self
            .toolbox
            .as_ref()
            .ok_or(Error::Forbidden)?
            .availability();
        let unique =
            |v: &Vec<String>| v.iter().collect::<std::collections::BTreeSet<_>>().len() == v.len();
        if input.allowed_tools.len() > 10
            || !unique(&input.allowed_tools)
            || input
                .allowed_tools
                .iter()
                .any(|n| !g.allowed_tools.contains(n))
            || input.programs.len() > 32
            || !unique(&input.programs)
            || input
                .programs
                .iter()
                .any(|p| !available.programs.contains(p))
            || input.mcp_connections.len() > 8
        {
            return Err(bad());
        }
        let native = input.allowed_tools.iter().any(|n| {
            matches!(
                n.as_str(),
                "execute_program" | "browser" | "office" | "github"
            )
        });
        if native && !input.explicitly_allow_native_owner_rights {
            return Err(Error::Forbidden);
        }
        if input.allowed_tools.iter().any(|n| n == "execute_program") && input.programs.is_empty()
            || input.allowed_tools.iter().any(|n| n == "browser") && !available.browser_available
            || input.allowed_tools.iter().any(|n| n == "office") && !available.office_available
        {
            return Err(bad());
        }
        let github_credential = match input.github_token {
            Some(secret) => Some(
                self.offline_seal_tool_secret(g, "github", "https://api.github.com", secret)
                    .await?,
            ),
            None => None,
        };
        if input.allowed_tools.iter().any(|n| n == "github")
            && (github_credential.is_none() || !available.programs.iter().any(|p| p == "gh"))
        {
            return Err(bad());
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut mcp_connections = Vec::new();
        for input in input.mcp_connections {
            valid_text(&input.id, 80)?;
            if !input
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                || !seen.insert(input.id.clone())
                || input.allowed_tools.is_empty()
                || input.allowed_tools.len() > 32
                || !unique(&input.allowed_tools)
                || input.bearer.is_some() == input.explicitly_allow_no_key
            {
                return Err(bad());
            }
            let secret = input.bearer.map(Zeroizing::new);
            let endpoint =
                cowork_mcp::Endpoint::new(&input.endpoint, secret.as_ref().map(|s| s.as_str()))
                    .map_err(|_| bad())?;
            // This explicit owner setup contacts only the user's selected MCP service.
            let mut session = cowork_mcp::Session::connect(endpoint, cowork_mcp::Limits::default())
                .await
                .map_err(|_| Error::Provider("offline_mcp_setup_connection_failed".into()))?;
            let catalog = session
                .discover()
                .await
                .map_err(|_| Error::Provider("offline_mcp_catalog_unavailable".into()))?;
            let definitions = catalog
                .tools()
                .filter(|(name, _)| input.allowed_tools.iter().any(|n| n.as_str() == *name))
                .map(|(_, tool)| tool.definition().clone())
                .collect::<Vec<_>>();
            if definitions.len() != input.allowed_tools.len() {
                return Err(bad());
            }
            let credential = match secret {
                Some(secret) => Some(
                    self.offline_seal_tool_secret(
                        g,
                        &format!("mcp/{}", input.id),
                        &input.endpoint,
                        secret.to_string(),
                    )
                    .await?,
                ),
                None => None,
            };
            mcp_connections.push(OfflineMcpConnection {
                id: input.id,
                endpoint: input.endpoint,
                allowed_tools: input.allowed_tools,
                catalog_sha256: catalog.sha256().into(),
                definitions,
                credential,
                allow_no_key: input.explicitly_allow_no_key,
            });
        }
        if input.allowed_tools.iter().any(|n| n == "mcp_call") && mcp_connections.is_empty() {
            return Err(bad());
        }
        self.store
            .set_offline_tool_configuration(
                g.id,
                &OfflineToolConfiguration {
                    allowed_tools: input.allowed_tools,
                    programs: input.programs,
                    github_credential,
                    mcp_connections,
                    native_owner_rights: input.explicitly_allow_native_owner_rights,
                },
            )
            .await?;
        self.offline_tool_configuration(g.id).await
    }
    pub async fn offline_tool_configuration(
        &self,
        grant: uuid::Uuid,
    ) -> Result<OfflineToolConfigurationView> {
        let ticket = self.store.offline_local_ticket(grant).await?;
        let config = self.store.offline_tool_configuration(grant).await?;
        let available = self
            .toolbox
            .as_ref()
            .ok_or(Error::Forbidden)?
            .availability();
        let digest = canonical_hash(&config);
        let configured = config.is_some();
        let c = config.unwrap_or_default();
        Ok(OfflineToolConfigurationView {
            grant_id: grant,
            configured,
            allowed_tools: configured_tools(&ticket.grant, Some(&c)),
            programs: c.programs,
            available_programs: available.programs,
            browser_available: available.browser_available,
            office_available: available.office_available,
            github_credential_configured: c.github_credential.is_some(),
            mcp_connections: c
                .mcp_connections
                .into_iter()
                .map(|m| OfflineMcpConnectionView {
                    id: m.id,
                    endpoint: m.endpoint,
                    allowed_tools: m.allowed_tools,
                    catalog_sha256: m.catalog_sha256,
                    definitions: m.definitions,
                    credential_configured: m.credential.is_some() || m.allow_no_key,
                })
                .collect(),
            configuration_sha256: digest,
            filesystem_isolation: false,
        })
    }
    pub(super) async fn offline_toolbox(
        &self,
        g: &OfflineGrant,
        run: uuid::Uuid,
    ) -> Result<Arc<Toolbox>> {
        if let Some(service) = self
            .offline_workers
            .services
            .lock()
            .map_err(|_| Error::Conflict)?
            .get(&run)
            .cloned()
        {
            return Ok(service);
        }
        let c = self
            .store
            .offline_tool_configuration(g.id)
            .await?
            .unwrap_or_default();
        let github = match c.github_credential.as_ref() {
            Some(sealed) => Some(
                self.offline_open_tool_secret(g, "github", "https://api.github.com", sealed)
                    .await?
                    .to_string(),
            ),
            None => None,
        };
        let service = self
            .toolbox
            .as_ref()
            .ok_or(Error::Forbidden)?
            .fork_offline(&g.id.to_string(), github)
            .map_err(|_| bad())?;
        let mut services = self
            .offline_workers
            .services
            .lock()
            .map_err(|_| Error::Conflict)?;
        if services.len() >= 32 && !services.contains_key(&run) {
            return Err(Error::Validation("offline_close_unused_runs_first".into()));
        }
        Ok(services.entry(run).or_insert(service).clone())
    }
    pub async fn offline_workspace_file(&self, run: uuid::Uuid, path: String) -> Result<Vec<u8>> {
        let e = self.store.offline_local_execution(run).await?;
        if !e.ticket.grant.allowed_tools.iter().any(|n| {
            matches!(
                n.as_str(),
                "office"
                    | "read_workspace_file"
                    | "write_workspace_file"
                    | "execute_program"
                    | "browser"
                    | "github"
            )
        }) {
            return Err(Error::Forbidden);
        }
        let service = self.offline_toolbox(&e.ticket.grant, run).await?;
        service
            .read_output_file(&run.to_string(), &path)
            .map_err(|_| bad())
    }
    pub async fn offline_workspace_upload(
        &self,
        run: uuid::Uuid,
        input: OfflineWorkspaceUpload,
    ) -> Result<OfflineRun> {
        let mut e = self.store.offline_local_execution(run).await?;
        if e.run.state != "awaiting_approval"
            || e.revoked
            || input.bytes.len() > 16 * 1024 * 1024
            || !e
                .ticket
                .grant
                .allowed_tools
                .iter()
                .any(|n| matches!(n.as_str(), "office" | "write_workspace_file"))
        {
            return Err(Error::Forbidden);
        }
        let service = self.offline_toolbox(&e.ticket.grant, run).await?;
        let digest = hash(&input.bytes);
        let old = e.run.revision;
        e.run.revision += 1;
        // Reserve this exact upload across its file effect. Approval/configuration
        // cannot interleave until its completion receipt restores awaiting_approval.
        e.run.state = "running".into();
        cowork_storage::offline_execution::append_offline_journal(
            &mut e.run,
            "owner_workspace_upload_dispatched",
            json!({"path":input.path,"size":input.bytes.len(),"sha256":digest,"automatic_replay":false}),
            now(),
        )?;
        self.store
            .save_offline_run(&e.run, &e.messages, old)
            .await?;
        let result = service.write_input_file(&run.to_string(), &input.path, &input.bytes);
        let old = e.run.revision;
        e.run.revision += 1;
        if result.is_err() {
            e.run.state = "unknown".into();
            e.run.error = Some("offline_workspace_upload_unconfirmed_no_replay".into());
            e.run.finished_at = Some(now());
        } else {
            e.run.state = "awaiting_approval".into();
        }
        cowork_storage::offline_execution::append_offline_journal(
            &mut e.run,
            if result.is_ok() {
                "owner_workspace_upload_confirmed"
            } else {
                "owner_workspace_upload_unknown"
            },
            json!({"path":input.path,"sha256":digest}),
            now(),
        )?;
        self.store
            .save_offline_run(&e.run, &e.messages, old)
            .await?;
        result.map_err(|_| Error::UncertainEffect)?;
        Ok(e.run)
    }
    pub(super) async fn offline_external_tool(
        &self,
        g: &OfflineGrant,
        run: uuid::Uuid,
        tool: &OfflinePendingTool,
        deadline: i64,
        stop: &CancellationToken,
    ) -> Result<Value> {
        let c = self
            .store
            .offline_tool_configuration(g.id)
            .await?
            .ok_or(Error::Forbidden)?;
        if !c.allowed_tools.contains(&tool.name) {
            return Err(Error::Forbidden);
        }
        if tool.name == "mcp_call" {
            return self
                .execute_offline_mcp_call(g, run, &c, tool, deadline, stop)
                .await;
        }
        let service = self.offline_toolbox(g, run).await?;
        let order = run.to_string();
        let a = &tool.arguments;
        let text = |name: &str| {
            a.get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(bad)
        };
        match tool.name.as_str() {
            "write_workspace_file" => {
                let path = text("path")?;
                let content = text("content")?;
                if a.as_object().is_none_or(|o| o.len() != 2)
                    || content.len() > 1024 * 1024
                    || content.contains('\0')
                {
                    return Err(bad());
                }
                service
                    .write_input_file(&order, &path, content.as_bytes())
                    .map_err(|_| Error::UncertainEffect)?;
                return Ok(
                    json!({"path":path,"size":content.len(),"sha256":hash(content.as_bytes())}),
                );
            }
            "read_workspace_file" => {
                let path = text("path")?;
                if a.as_object().is_none_or(|o| o.len() != 1) {
                    return Err(bad());
                }
                let bytes = service.read_output_file(&order, &path).map_err(|_| bad())?;
                if bytes.len() > 1024 * 1024 {
                    return Err(bad());
                }
                return Ok(
                    json!({"path":path,"size":bytes.len(),"sha256":hash(&bytes),"content":std::str::from_utf8(&bytes).ok()}),
                );
            }
            _ => {}
        }
        if !c.native_owner_rights {
            return Err(Error::Forbidden);
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Program {
            program: String,
            arguments: Vec<String>,
            timeout_seconds: u64,
            #[serde(default)]
            cwd: String,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Browser {
            #[serde(default)]
            engine: Option<BrowserEngine>,
            #[serde(default)]
            mode: Option<BrowserMode>,
            #[serde(default)]
            connection_id: Option<String>,
            #[serde(default)]
            session_id: Option<String>,
            operation: String,
            #[serde(default)]
            url: Option<String>,
            #[serde(default)]
            selector: Option<String>,
            #[serde(default)]
            text: Option<String>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Office {
            operation: String,
            #[serde(default)]
            input: Option<String>,
            #[serde(default)]
            output: Option<String>,
            #[serde(default)]
            text: Option<String>,
            #[serde(default)]
            title: Option<String>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Github {
            arguments: Vec<String>,
        }
        let (action, seconds) = match tool.name.as_str() {
            "execute_program" => {
                let p: Program = serde_json::from_value(a.clone()).map_err(|_| bad())?;
                if !c.programs.contains(&p.program) || !(1..=600).contains(&p.timeout_seconds) {
                    return Err(Error::Forbidden);
                }
                (
                    Action::Terminal {
                        program: p.program,
                        arguments: p.arguments,
                        cwd: p.cwd,
                    },
                    p.timeout_seconds,
                )
            }
            "browser" => {
                let b: Browser = serde_json::from_value(a.clone()).map_err(|_| bad())?;
                let session_id = match b.session_id {
                    Some(s) => s,
                    None if b.engine.is_some() || b.mode.is_some() => {
                        service
                            .create_browser_v2(
                                &g.project_id.to_string(),
                                &order,
                                CreateBrowserSession {
                                    engine: b.engine.unwrap_or_default(),
                                    mode: b.mode.unwrap_or_default(),
                                    connection_id: b.connection_id,
                                },
                            )
                            .map_err(|_| bad())?
                            .id
                    }
                    None if service.selected_browser_session(&order).is_some() => {
                        service.selected_browser_session(&order).ok_or_else(bad)?
                    }
                    None => {
                        service
                            .create_browser_session(&order)
                            .map_err(|_| bad())?
                            .id
                    }
                };
                (
                    Action::Browser {
                        session_id,
                        operation: b.operation,
                        url: b.url,
                        selector: b.selector,
                        text: b.text,
                    },
                    120,
                )
            }
            "office" => {
                let o: Office = serde_json::from_value(a.clone()).map_err(|_| bad())?;
                (
                    Action::Office {
                        operation: o.operation,
                        input: o.input,
                        output: o.output,
                        text: o.text,
                        title: o.title,
                    },
                    120,
                )
            }
            "github" => {
                let h: Github = serde_json::from_value(a.clone()).map_err(|_| bad())?;
                if c.github_credential.is_none() {
                    return Err(Error::Forbidden);
                }
                (
                    Action::Github {
                        arguments: h.arguments,
                    },
                    120,
                )
            }
            _ => return Err(Error::Forbidden),
        };
        let remaining = (deadline - now()).max(0) as u64 / 1000;
        if remaining == 0 || stop.is_cancelled() {
            return Err(Error::Forbidden);
        }
        let browser_session = match &action {
            Action::Browser { session_id, .. } => Some(session_id.clone()),
            _ => None,
        };
        let mut request = Request {
            idempotency_key: format!("offline-{}", tool.request_sha256),
            action,
            timeout_seconds: seconds.min(remaining),
            output_bytes: 128 * 1024,
            approval_sha256: None,
        };
        let preview = service.preview(&order, &request).map_err(|_| bad())?;
        request.approval_sha256 = Some(preview.request_sha256);
        let job = service
            .submit(&order, request)
            .map_err(|_| Error::UncertainEffect)?;
        struct Guard {
            service: Arc<Toolbox>,
            order: String,
            id: String,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = self.service.cancel(&self.order, &self.id);
            }
        }
        let _guard = Guard {
            service: service.clone(),
            order: order.clone(),
            id: job.id.clone(),
        };
        loop {
            let job = service
                .status(&order, &job.id)
                .map_err(|_| Error::UncertainEffect)?;
            if job.state == "completed" {
                let outcome = job.outcome.as_ref().ok_or(Error::UncertainEffect)?;
                if outcome.exit_code.is_none() || !outcome.output_complete {
                    return Err(Error::UncertainEffect);
                }
                let capture = |bytes: &[u8]| {
                    String::from_utf8_lossy(&bytes[..bytes.len().min(24 * 1024)]).into_owned()
                };
                return Ok(
                    json!({"job_id":job.id,"state":job.state,"workspace":preview.cwd,"browser_session_id":browser_session,"exit_code":outcome.exit_code,"termination":outcome.termination,"stdout":capture(&outcome.stdout.bytes),"stderr":capture(&outcome.stderr.bytes),"stdout_truncated":outcome.stdout.truncated||outcome.stdout.bytes.len()>24*1024,"stderr_truncated":outcome.stderr.truncated||outcome.stderr.bytes.len()>24*1024,"output_complete":outcome.output_complete,"isError":outcome.exit_code!=Some(0)}),
                );
            }
            if job.state != "running" || now() >= deadline {
                return Err(Error::UncertainEffect);
            }
            tokio::select! {_=stop.cancelled()=>return Err(Error::UncertainEffect),_=tokio::time::sleep(Duration::from_millis(100))=>{}}
        }
    }
}
