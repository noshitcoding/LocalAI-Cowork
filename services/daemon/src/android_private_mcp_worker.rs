//! Private MCP sessions have their own actor/project custody. No native-owner runtime is used.
use super::*;
use cowork_contracts::{McpToolBinding, RunSpec, ToolCall};
use cowork_domain::canonical_hash;
use cowork_mcp::{CheckpointScope, Endpoint, Limits, SealedSession, Session, SessionCipher};
use cowork_storage::android_private_mcp_runtime::PrivateMcpGate;
use std::collections::BTreeMap;
use uuid::Uuid;
#[path = "android_private_mcp_task_worker.rs"]
mod tasks;

pub(super) struct PrivateMcpSession {
    session: Session,
    frozen: Value,
    transport: String,
    revision: i64,
    isolated: Option<PrivateIsolatedMcp>,
}
struct PrivateIsolatedMcp {
    container: super::private_runner::Container,
    bridge: super::private_stdio::Bridge,
    binding_sha256: String,
}
fn failed() -> Error {
    Error::UncertainEffect
}
impl AndroidDeviceServices {
    async fn private_mcp_cipher(&self, original: Option<KeyId>) -> Result<SessionCipher> {
        static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
        let slot = SLOTS.acquire().await.map_err(|_| failed())?;
        let keys = self.keys.clone();
        let active = self.key_id;
        let pending = tokio::task::spawn_blocking(move || {
            let _slot = slot;
            let id = original
                .map(Ok)
                .unwrap_or_else(|| {
                    keys.key_for_purpose(&active, cowork_secrets::KeyPurpose::DataEncryption)
                })
                .map_err(|_| Error::NativeKeyUnavailable)?;
            SessionCipher::new(
                &id.cipher_id().map_err(|_| Error::NativeKeyUnavailable)?,
                keys.load(&id).map_err(|_| Error::NativeKeyUnavailable)?,
            )
            .map_err(|_| failed())
        });
        // A timed-out native lookup retains its bounded slot until that lookup really returns.

        tokio::time::timeout(Duration::from_secs(10), pending)
            .await
            .map_err(|_| Error::NativeKeyUnavailable)?
            .map_err(|_| failed())?
    }
    pub(super) async fn private_mcp_wait<T, F>(
        &self,
        run: Uuid,
        lease: Uuid,
        discovery: Option<i64>,
        stop: &CancellationToken,
        future: F,
    ) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        tokio::pin!(future);
        let deadline = tokio::time::sleep(Duration::from_secs(120));
        tokio::pin!(deadline);
        let mut pulse = tokio::time::interval(Duration::from_secs(3));
        pulse.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {biased;_=stop.cancelled()=>return Err(failed()),_=&mut deadline=>return Err(failed()),_=pulse.tick()=>{self.store.android_private_server_renew(run,lease).await?;if let Some(sequence)=discovery{self.store.android_private_mcp_discovery_validate(run,lease,sequence,self.keys.as_ref()).await?;}else{self.store.android_private_loop_checkpoint(run,lease,self.keys.as_ref()).await?;}},result=&mut future=>return result}
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "MCP continuation must bind the run/project/entity and frozen revision to the original transport session, checkpoint cipher and optional isolated process."
    )]
    fn private_mcp_continuation(
        run: Uuid,
        project: Uuid,
        entity: Uuid,
        frozen: &Value,
        revision: i64,
        transport: &str,
        session: &Session,
        cipher: &SessionCipher,
        isolated: Option<&PrivateIsolatedMcp>,
    ) -> Result<Value> {
        if transport == "stdio" {
            let original = isolated.ok_or_else(failed)?;
            return Ok(
                json!({"transport":"stdio","connection_id":session.connection_id(),"epoch":session.epoch(),"sealed":null,"isolated":{"nonce":original.container.nonce,"container_id":original.container.physical_id,"binding_sha256":original.binding_sha256}}),
            );
        }
        if transport == "sse" {
            return Ok(
                json!({"transport":"sse","connection_id":session.connection_id(),"epoch":session.epoch(),"sealed":null}),
            );
        }
        let scope = CheckpointScope {
            project_id: project.to_string(),
            run_id: run.to_string(),
            server_id: entity.to_string(),
            connection_record_id: format!("{run}:{entity}"),
            server_revision: frozen["revision"].as_i64().ok_or_else(invalid)?,
            checkpoint_revision: revision,
            configuration_sha256: frozen["binding_sha256"]
                .as_str()
                .ok_or_else(invalid)?
                .into(),
            catalog_sha256: session.catalog().ok_or_else(failed)?.sha256().into(),
        };
        let sealed = cipher.seal(session, &scope).map_err(|_| failed())?;
        Ok(
            json!({"transport":"streamable_http","connection_id":session.connection_id(),"epoch":session.epoch(),"scope":scope,"sealed":sealed}),
        )
    }
    pub(super) async fn private_mcp_sessions(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        stop: &CancellationToken,
    ) -> Result<(BTreeMap<String, PrivateMcpSession>, Vec<McpToolBinding>)> {
        let selected = lease.value["spec"]["input"]["frozen_mcp_bindings"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let project = Uuid::parse_str(
            lease.value["spec"]["project_id"]
                .as_str()
                .ok_or_else(invalid)?,
        )
        .map_err(|_| invalid())?;
        let mut sessions = BTreeMap::new();
        let mut bindings = Vec::new();
        for frozen in selected {
            if stop.is_cancelled() {
                return Err(failed());
            }
            let entity = Uuid::parse_str(frozen["entity_id"].as_str().ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
            let config = self
                .store
                .android_private_mcp_open_for_run(
                    lease.run,
                    lease.lease,
                    &frozen,
                    self.keys.as_ref(),
                )
                .await?;
            let mut c = cowork_storage::android_private_mcp::configuration(&config)?;
            if let Some(oauth) = &c.oauth {
                let owner = Uuid::parse_str(
                    lease.value["spec"]["creator_user_id"]
                        .as_str()
                        .ok_or_else(invalid)?,
                )
                .map_err(|_| invalid())?;
                let credential = self
                    .store
                    .mcp_oauth_bearer_for(
                        &cowork_storage::mcp_oauth::McpOAuthOwner::PersonalExecution {
                            owner,
                            run: lease.run,
                            lease: lease.lease,
                            key: self.key_id,
                        },
                        &oauth.id,
                        self.keys.as_ref(),
                    )
                    .await?;
                c.headers.insert(
                    "authorization".into(),
                    format!("Bearer {}", credential.as_str()),
                );
            }
            let mut isolated = None;
            let previous = self
                .store
                .android_private_mcp_connection(lease.run, lease.lease, entity, self.keys.as_ref())
                .await?;
            let (session, revision) = if let Some(previous) = previous {
                if c.transport != "streamable_http"
                    || previous.checkpoint["transport"] != "streamable_http"
                {
                    return Err(failed());
                }
                let sealed: SealedSession =
                    serde_json::from_value(previous.checkpoint["sealed"].clone())
                        .map_err(|_| failed())?;
                let id = KeyId::from_cipher_id(sealed.key_id()).map_err(|_| failed())?;
                let cipher = self.private_mcp_cipher(Some(id)).await?;
                let expected = previous.scope(
                    project,
                    lease.run,
                    frozen["revision"].as_i64().ok_or_else(invalid)?,
                    previous.revision,
                );
                if previous.checkpoint["scope"] != json!(expected) {
                    return Err(failed());
                }
                let endpoint = Endpoint::public_https(&c.url, &c.headers, false)
                    .await
                    .map_err(|_| failed())?;
                let session = cipher
                    .open(
                        &sealed,
                        &expected,
                        endpoint,
                        Limits::default(),
                        previous.definitions,
                    )
                    .map_err(|_| failed())?;
                if session.connection_id()
                    != previous.checkpoint["connection_id"]
                        .as_str()
                        .ok_or_else(failed)?
                    || Some(session.epoch()) != previous.checkpoint["epoch"].as_u64()
                {
                    return Err(failed());
                }
                (session, previous.revision)
            } else {
                let (sequence, request_sha, admitted_revision) = self
                    .store
                    .android_private_mcp_discovery_admit(
                        lease.run,
                        lease.lease,
                        entity,
                        self.keys.as_ref(),
                        self.key_id,
                    )
                    .await?;
                let connect = async {
                    let (endpoint, isolated) = if c.transport == "stdio" {
                        let runner = self.private_runner.as_ref().ok_or(Error::Forbidden)?;
                        if lease.value["spec"]["input"]["frozen_private_runner"] != runner.binding()
                        {
                            return Err(Error::Forbidden);
                        }
                        let files = self
                            .store
                            .android_private_runner_initial_files(
                                lease.run,
                                lease.lease,
                                self.keys.as_ref(),
                            )
                            .await?;
                        let bootstrap = json!({"format":1,"mode":"mcp","command":c.command,"arguments":c.args,"environment":c.environment,"files":files});
                        let mut container = runner
                            .start(
                                lease.run,
                                lease.lease,
                                &if admitted_revision == 1 {
                                    format!("stdio:{entity}")
                                } else {
                                    format!("stdio:{entity}:{sequence}")
                                },
                                &bootstrap,
                                stop,
                            )
                            .await?;
                        let (bridge, endpoint) =
                            super::private_stdio::Bridge::attach(&mut container)?;
                        (
                            endpoint,
                            Some(PrivateIsolatedMcp {
                                container,
                                bridge,
                                binding_sha256: canonical_hash(&runner.binding()),
                            }),
                        )
                    } else {
                        (
                            Endpoint::public_https(&c.url, &c.headers, c.transport == "sse")
                                .await
                                .map_err(|_| failed())?,
                            None,
                        )
                    };
                    self.store
                        .android_private_mcp_discovery_validate(
                            lease.run,
                            lease.lease,
                            sequence,
                            self.keys.as_ref(),
                        )
                        .await?;
                    let mut session = Session::connect_interactive(endpoint, Limits::default())
                        .await
                        .map_err(|_| failed())?;
                    session.discover().await.map_err(|_| failed())?;
                    Ok((session, isolated))
                };
                let (session, own_isolated) = self
                    .private_mcp_wait(lease.run, lease.lease, Some(sequence), stop, connect)
                    .await?;
                isolated = own_isolated;
                let catalog = session.catalog().ok_or_else(failed)?;
                let definitions: Vec<Value> = catalog
                    .tools()
                    .map(|(_, d)| d.definition().clone())
                    .collect();
                let received = json!({"format":1,"catalog_sha256":catalog.sha256(),"definitions":definitions,"connection_id":session.connection_id(),"epoch":session.epoch(),"protocol":session.protocol_version()});
                self.store
                    .android_private_mcp_retain_received(
                        lease.run,
                        lease.lease,
                        sequence,
                        &request_sha,
                        &received,
                        true,
                        self.keys.as_ref(),
                        self.key_id,
                    )
                    .await?;
                if let Some(own) = &isolated {
                    let paused = self
                        .private_mcp_wait(
                            lease.run,
                            lease.lease,
                            Some(sequence),
                            stop,
                            own.bridge.workspace("capture", None),
                        )
                        .await?;
                    if paused["server_alive"] != true {
                        return Err(failed());
                    }
                }
                let cipher = self.private_mcp_cipher(None).await?;
                let continuation = Self::private_mcp_continuation(
                    lease.run,
                    project,
                    entity,
                    &frozen,
                    admitted_revision
                        .checked_add(1)
                        .ok_or(Error::BudgetExceeded)?,
                    &c.transport,
                    &session,
                    &cipher,
                    isolated.as_ref(),
                )?;
                self.store
                    .android_private_mcp_discovery_received(
                        lease.run,
                        lease.lease,
                        entity,
                        sequence,
                        &request_sha,
                        &received,
                        &continuation,
                        self.keys.as_ref(),
                        self.key_id,
                    )
                    .await?;
                (
                    session,
                    admitted_revision
                        .checked_add(1)
                        .ok_or(Error::BudgetExceeded)?,
                )
            };
            let catalog = session.catalog().ok_or_else(failed)?;
            for (name, definition) in catalog.tools() {
                bindings.push(McpToolBinding {
                    alias: cowork_domain::mcp::mcp_alias(&entity.to_string(), name),
                    server_id: entity.to_string(),
                    server_revision: frozen["revision"].as_i64().ok_or_else(invalid)?,
                    endpoint: isolated
                        .as_ref()
                        .map(|own| format!("cowork-isolated-stdio://{}", own.container.nonce))
                        .unwrap_or_else(|| c.url.clone()),
                    configuration_sha256: frozen["binding_sha256"]
                        .as_str()
                        .ok_or_else(invalid)?
                        .into(),
                    catalog_sha256: catalog.sha256().into(),
                    definition: definition.definition().clone(),
                });
            }
            if bindings.len() > 128 {
                return Err(Error::BudgetExceeded);
            }
            sessions.insert(
                entity.to_string(),
                PrivateMcpSession {
                    session,
                    frozen,
                    transport: c.transport.clone(),
                    revision,
                    isolated,
                },
            );
        }
        Ok((sessions, bindings))
    }
    pub(super) async fn private_mcp_tool(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        cp: &Value,
        sessions: &mut BTreeMap<String, PrivateMcpSession>,
        stop: &CancellationToken,
    ) -> Result<bool> {
        match self
            .store
            .android_private_mcp_gate(lease.run, lease.lease, self.keys.as_ref(), self.key_id)
            .await?
        {
            PrivateMcpGate::Waiting(_) => return Ok(false),
            PrivateMcpGate::Denied(_) => return Ok(true),
            PrivateMcpGate::Ready => {}
        }
        let tool: ToolCall =
            serde_json::from_value(cp["pending_tools"][0].clone()).map_err(|_| invalid())?;
        let spec: RunSpec = serde_json::from_value(cp["spec"].clone()).map_err(|_| invalid())?;
        let binding = spec
            .mcp_tools
            .iter()
            .find(|b| b.alias == tool.name)
            .ok_or(Error::Forbidden)?;
        let active = sessions.get_mut(&binding.server_id).ok_or_else(failed)?;
        if let Some(own) = &active.isolated {
            let files = self
                .store
                .android_private_runner_files(lease.run, lease.lease, self.keys.as_ref())
                .await?;
            self.store
                .android_private_container_live(
                    lease.run,
                    lease.lease,
                    own.container.nonce,
                    self.keys.as_ref(),
                )
                .await?;
            self.private_mcp_wait(
                lease.run,
                lease.lease,
                None,
                stop,
                own.bridge.workspace("prepare", Some(&files)),
            )
            .await?;
        }
        let prepared = active
            .session
            .prepare(
                &binding.catalog_sha256,
                binding.definition["name"].as_str().ok_or_else(invalid)?,
                tool.arguments,
            )
            .map_err(|_| failed())?;
        let permit = self
            .store
            .android_private_mcp_admit(
                lease.run,
                lease.lease,
                prepared,
                self.keys.as_ref(),
                self.key_id,
            )
            .await?;
        let sequence = permit.sequence;
        let request_sha = permit.request_sha256.clone();
        let entity = permit.entity;
        self.store
            .android_private_mcp_validate_dispatch(
                lease.run,
                lease.lease,
                sequence,
                self.keys.as_ref(),
            )
            .await?;
        let result = self
            .private_mcp_wait(lease.run, lease.lease, None, stop, async {
                Ok(active.session.call(permit.into_prepared()).await)
            })
            .await;
        match result {
            Ok(Ok(result)) => {
                let result = if result.task.is_some() {
                    self.private_mcp_task_await(lease, sequence, active, result, stop)
                        .await?
                } else {
                    result
                };
                let received = json!({"format":1,"response":result.response,"response_sha256":result.response_sha256,"catalog_changed":result.catalog_changed,"uncertain":false});
                self.store
                    .android_private_mcp_retain_received(
                        lease.run,
                        lease.lease,
                        sequence,
                        &request_sha,
                        &received,
                        true,
                        self.keys.as_ref(),
                        self.key_id,
                    )
                    .await?;
                let mut server_available = true;
                if let Some(own) = &active.isolated {
                    let control_id = Uuid::new_v4();
                    let capture = self
                        .store
                        .android_private_mcp_workspace_admit(
                            lease.run,
                            lease.lease,
                            entity,
                            sequence,
                            control_id,
                            self.keys.as_ref(),
                            self.key_id,
                        )
                        .await?;
                    if capture.nonce != own.container.nonce {
                        return Err(failed());
                    }
                    self.store
                        .android_private_mcp_workspace_validate(
                            lease.run,
                            lease.lease,
                            capture.sequence,
                            self.keys.as_ref(),
                        )
                        .await?;
                    let workspace = self
                        .private_mcp_wait(
                            lease.run,
                            lease.lease,
                            None,
                            stop,
                            own.bridge.workspace_original(control_id, "capture", None),
                        )
                        .await?;
                    server_available = workspace["server_alive"] == true;
                    self.store
                        .android_private_mcp_retain_received(
                            lease.run,
                            lease.lease,
                            capture.sequence,
                            &capture.request_sha256,
                            &workspace,
                            true,
                            self.keys.as_ref(),
                            self.key_id,
                        )
                        .await?;
                }
                let continuation = if !result.catalog_changed && server_available {
                    let cipher = self.private_mcp_cipher(None).await?;
                    Some(Self::private_mcp_continuation(
                        lease.run,
                        Uuid::parse_str(&spec.project_id).map_err(|_| invalid())?,
                        entity,
                        &active.frozen,
                        active.revision + 1,
                        &active.transport,
                        &active.session,
                        &cipher,
                        active.isolated.as_ref(),
                    )?)
                } else {
                    None
                };
                self.store
                    .android_private_mcp_call_received(
                        lease.run,
                        lease.lease,
                        entity,
                        sequence,
                        &request_sha,
                        &received,
                        continuation.as_ref(),
                        self.keys.as_ref(),
                        self.key_id,
                    )
                    .await?;
                active.revision += 1;
                Ok(true)
            }
            Ok(Err(failure)) => {
                let received = json!({"format":1,"uncertain":true,"code":failure.code.to_string(),"response":failure.response(),"response_sha256":failure.response().map(cowork_domain::canonical_hash)});
                let _ = self
                    .store
                    .android_private_mcp_retain_received(
                        lease.run,
                        lease.lease,
                        sequence,
                        &request_sha,
                        &received,
                        false,
                        self.keys.as_ref(),
                        self.key_id,
                    )
                    .await;
                Err(failed())
            }
            Err(error) => {
                let received = json!({"format":1,"uncertain":true,"code":"mcp_effect_uncertain"});
                let _ = self
                    .store
                    .android_private_mcp_retain_received(
                        lease.run,
                        lease.lease,
                        sequence,
                        &request_sha,
                        &received,
                        false,
                        self.keys.as_ref(),
                        self.key_id,
                    )
                    .await;
                Err(error)
            }
        }
    }
    pub(super) async fn private_mcp_close(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        sessions: &mut BTreeMap<String, PrivateMcpSession>,
    ) -> Result<()> {
        let mut error = None;
        for (entity, active) in sessions.iter_mut() {
            if let Some(own) = &mut active.isolated {
                let prepared = self
                    .store
                    .android_private_mcp_prepare_close(
                        lease.run,
                        lease.lease,
                        Uuid::parse_str(entity).map_err(|_| invalid())?,
                        active.revision,
                        &active.session.connection_id(),
                        active.session.epoch(),
                        self.keys.as_ref(),
                        self.key_id,
                    )
                    .await;
                // Cleanup of our held original process remains necessary after authority loss.
                // Failure to journal or remove never becomes a positive closure receipt.
                let bridge = own.bridge.close().await;
                let physical = own.container.finish().await;
                match (prepared, bridge, physical) {
                    (Ok(permit), Ok(()), Ok(_)) => {
                        if let Err(e) = self
                            .store
                            .android_private_mcp_finish_close(
                                permit,
                                self.keys.as_ref(),
                                self.key_id,
                            )
                            .await
                        {
                            error.get_or_insert(e);
                        }
                    }
                    (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                        error.get_or_insert(e);
                    }
                }
            }
        }
        sessions.clear();
        error.map_or(Ok(()), Err)
    }
}
