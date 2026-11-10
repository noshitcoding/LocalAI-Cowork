mod execution_plans;
mod execution_plans_v2;
mod file_operations;
pub use execution_plans_v2::managed::{ManagedInputAccess, ManagedInputManifest};
pub mod agent_config;
mod automation;
pub mod codex_accounts;
mod codex_profiles;
mod document_actions;
mod document_service;
mod document_tools;
mod execution_inputs;
pub mod external_memory;
mod files;
mod folder_context;
pub mod historical_imports;
mod managed_results;
mod mcp;
mod mcp_interactions;
mod mcp_lifecycle;
mod mcp_native;
pub mod mcp_oauth;
mod mcp_sessions;
pub mod mcp_tasks;
mod native_auth_transport;
mod native_recovery;
mod native_secrets;
pub mod offline_execution;
mod offline_mcp_interactions;
mod offline_mcp_tasks;
mod offline_tools;
pub mod personal_provider_credentials;
mod processes;
mod provider_settings;
mod toolbox_bridge;
mod workflow_documents;
pub use processes::{ManagedResultDeliveryResponse, ProcessRunnerAccess, ProcessRunnerConfig};
mod model_context;
mod workflow_coordinator;
mod workflow_external;
mod workflow_journal;
mod workflow_mcp_tasks;
mod workflow_models;
mod workflow_product;
mod workflow_recovery;
pub use mcp_sessions::{McpSecretAccess, McpSessionAccess};
pub mod provider;
mod restore;
mod uploads;
mod workspaces;
use cowork_contracts::*;
use cowork_domain::*;
use cowork_policy::{Decision, evaluate};
use cowork_storage::{Change, Store, id, now};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{RwLock, mpsc},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredProvider {
    pub profile: ProviderProfile,
    /// Credential value never crosses API contracts or enters a run snapshot.
    pub credential_env: Option<String>,
}
#[derive(Clone)]
pub struct Runtime {
    pub store: Store,
    pub providers: Arc<RwLock<Vec<ConfiguredProvider>>>,
    pub allow_test: bool,
    pub files: Option<Arc<cowork_broker::FileBroker>>,
    upload_transfers: Arc<tokio::sync::Semaphore>,
    native_keys: Option<Arc<dyn cowork_secrets::NativeKeyProvider>>,
    personal_provider_keys: Option<personal_provider_credentials::PersonalProviderKeys>,
    process_runner: Option<Arc<ProcessRunnerConfig>>,
    toolbox: Option<Arc<cowork_toolbox::Toolbox>>,
    offline_workers: Arc<offline_execution::OfflineWorkers>,
    codex: Option<Arc<codex_accounts::CodexBroker>>,
}
impl Runtime {
    pub fn new(store: Store, providers: Vec<ConfiguredProvider>, allow_test: bool) -> Result<Self> {
        let mut ids = std::collections::HashSet::new();
        for p in &providers {
            provider::validate_profile(&p.profile, allow_test)?;
            if !ids.insert(p.profile.id.clone()) {
                return Err(Error::Validation("Duplicate provider id".into()));
            }
        }
        Ok(Self {
            store,
            providers: Arc::new(RwLock::new(providers)),
            allow_test,
            files: None,
            upload_transfers: Arc::new(tokio::sync::Semaphore::new(2)),
            native_keys: None,
            personal_provider_keys: None,
            process_runner: None,
            toolbox: None,
            offline_workers: Arc::new(Default::default()),
            codex: None,
        })
    }
    pub fn with_file_broker(mut self, broker: cowork_broker::FileBroker) -> Self {
        self.files = Some(Arc::new(broker));
        self
    }
    pub fn with_codex_data_dir(mut self, directory: std::path::PathBuf) -> Self {
        self.codex = Some(Arc::new(codex_accounts::CodexBroker::new(directory)));
        self
    }
    pub fn codex_broker(&self) -> Option<Arc<codex_accounts::CodexBroker>> {
        self.codex.clone()
    }
    pub fn with_native_key_provider(
        mut self,
        provider: Arc<dyn cowork_secrets::NativeKeyProvider>,
    ) -> Self {
        self.native_keys = Some(provider);
        self
    }
    pub async fn profiles(&self) -> Vec<ProviderProfile> {
        let configured = self.providers.read().await.clone();
        let mut profiles = Vec::with_capacity(configured.len());
        for p in &configured {
            let mut profile = p.profile.clone();
            profile.credential_ready = !profile.credential_required
                || self
                    .personal_provider_credential_from(p)
                    .await
                    .is_ok_and(|value| value.is_some());
            profiles.push(profile);
        }
        profiles
    }
    pub async fn preflight(&self, input: &CreateRun) -> Result<Preflight> {
        self.preflight_context_scope(input, true).await
    }
    pub(crate) async fn preflight_context_scope(
        &self,
        input: &CreateRun,
        check_context: bool,
    ) -> Result<Preflight> {
        valid_text(&input.prompt, 64_000)?;
        validate_budgets(&input.budgets)?;
        if let Some(config) = &input.agent_config {
            cowork_storage::library_config::validate_effective_agent_config(config)?;
        }
        self.store.native_chat_available(&input.order_id).await?;
        let order = self.store.order(&input.order_id).await?;
        if order.revision != input.base_revision {
            return Err(Error::Conflict);
        }
        let profile = self
            .profiles()
            .await
            .into_iter()
            .find(|p| p.id == input.provider_id)
            .ok_or(Error::Provider("profile_missing".into()))?;
        if profile.revision != input.provider_revision {
            return Err(Error::Conflict);
        }
        if !profile.credential_ready {
            return Err(Error::Provider("credential_missing".into()));
        }
        if (input.model_binding.is_none()
            && input.budgets.max_total_tokens < profile.context_tokens as u64)
            || input.budgets.max_output_tokens >= profile.context_tokens
        {
            return Err(Error::BudgetConfiguration);
        }
        let credential = self.personal_provider_credential(&profile).await?;
        self.probe_configured_provider(&profile, credential).await?;
        if !input.folders.is_empty() {
            self.files
                .as_ref()
                .ok_or(Error::Forbidden)?
                .validate_grants(&order.project_id, &input.folders)?;
        }
        let mut folders = input.folders.clone();
        if self.files.is_some() {
            folders.push(self.workspace(&order.id).await?.folder);
        }
        let mut allowed_tools = vec![
            "create_artifact".into(),
            "list_artifacts".into(),
            "read_artifact".into(),
            "request_input".into(),
        ];
        if !folders.is_empty() {
            allowed_tools.extend([
                "list_files".into(),
                "read_text_file".into(),
                "stat_file".into(),
            ]);
        }
        if folders.iter().any(|f| f.writable) {
            allowed_tools.extend([
                "write_text_file".into(),
                "create_directory".into(),
                "rename_file".into(),
                "remove_file".into(),
                "restore_file".into(),
            ]);
        }
        // Built-in document tools come with every chat once their pinned engine is ready.
        if !folders.is_empty() && self.document_tools_ready() {
            allowed_tools.push("document_read".into());
            if folders.iter().any(|f| f.writable) {
                allowed_tools.extend(["document_write".into(), "image_edit".into()]);
            }
        }
        let mut executor = "local-v2".to_owned();
        let environment = match &input.environment {
            Some(reference) => {
                if !folders
                    .iter()
                    .any(|f| f.writable && f.order_id.as_deref() == Some(order.id.as_str()))
                {
                    return Err(Error::Forbidden);
                }
                let (_, observation) = self
                    .process_environment(&order.project_id, reference)
                    .await?;
                self.require_sandbox_session_for_order(&order.project_id, &order.id, reference)
                    .await?;
                executor = observation.runner_id;
                allowed_tools.push("execute_program".into());
                Some(reference.clone())
            }
            None => None,
        };
        let native = self.selected_native_tools(&input.native_tools)?;
        if !native.is_empty() {
            if environment.is_some() {
                return Err(Error::Validation(
                    "Choose native tools or a qualified runner environment for this run".into(),
                ));
            }
            executor = "owner-native-toolbox-v1".into();
            allowed_tools.extend(native);
        }
        let flight = Preflight {
            model_binding: input.model_binding.clone(),
            environment,
            mcp_tools: self
                .store
                .select_mcp_tools(&order.project_id, &input.mcp_tools)
                .await?,
            ready: true,
            provider: profile,
            executor,
            policy_revision: 1,
            access_mode: input.access_mode.clone(),
            allowed_tools,
            budgets: input.budgets.clone(),
            folders,
        };
        let mut flight = flight;
        if !flight.mcp_tools.is_empty() {
            self.validate_owner_mcp_bindings(&order.project_id, &flight.mcp_tools)
                .await?;
            flight
                .allowed_tools
                .extend(flight.mcp_tools.iter().map(|b| b.alias.clone()));
        }
        if let Some(config) = &input.agent_config {
            cowork_domain::validate_agent_values(config)?;
            if let Some(filter) = &config.tool_filter {
                // Document tools are optional capabilities: a filter naming them while the
                // engine is not installed narrows the chat instead of rejecting it.
                if filter.iter().any(|tool| {
                    !flight.allowed_tools.contains(tool) && !cowork_domain::document_tool(tool)
                }) {
                    return Err(Error::Validation(
                        "A selected agent tool is unavailable in this chat".into(),
                    ));
                }
                flight.allowed_tools.retain(|tool| filter.contains(tool));
                flight
                    .mcp_tools
                    .retain(|binding| filter.contains(&binding.alias));
            }
        }
        if matches!(flight.access_mode, AccessMode::ReadOnly) {
            flight
                .allowed_tools
                .retain(|tool| cowork_domain::read_only_tool(tool));
            flight.mcp_tools.clear();
            for folder in &mut flight.folders {
                folder.writable = false;
            }
        }
        let spec = self.configured_run_spec(input, &order, &flight).await?;
        flight.access_mode = spec.access_mode.clone();
        flight.budgets = spec.budgets.clone();
        flight.allowed_tools = spec.allowed_tools.clone();
        flight.mcp_tools = spec.mcp_tools.clone();
        flight.folders = spec.folders.clone();
        let messages = self.store.preview_model_context(&spec).await?;
        if check_context {
            match self.prepare_model(&spec, &messages).await {
                // Automatically granted document tools never make a small context unusable.
                Err(Error::ContextLimit)
                    if flight
                        .allowed_tools
                        .iter()
                        .any(|t| cowork_domain::document_tool(t)) =>
                {
                    flight
                        .allowed_tools
                        .retain(|t| !cowork_domain::document_tool(t));
                    let spec = self.configured_run_spec(input, &order, &flight).await?;
                    flight.allowed_tools = spec.allowed_tools.clone();
                    let messages = self.store.preview_model_context(&spec).await?;
                    self.prepare_model(&spec, &messages).await?;
                }
                other => {
                    other?;
                }
            }
        } else {
            self.store.registered_model(&spec).await?;
        }
        Ok(flight)
    }
    pub async fn create_run(&self, input: &CreateRun) -> Result<RunRecord> {
        if let Some(existing) = self.store.existing_request(input).await? {
            if let Some(plan) = self
                .store
                .existing_execution_plan(&cowork_workflow_contracts::RunRequest::Single(
                    input.clone(),
                ))
                .await?
                && let Some(receipt) = self.store.execution_plan_run(&plan).await?
            {
                return Ok(receipt);
            }
            return Ok(existing);
        }
        let prepared = match self
            .prepare_execution_plan(&cowork_workflow_contracts::RunRequest::Single(
                input.clone(),
            ))
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                return match self.store.existing_request(input).await? {
                    Some(existing) => Ok(existing),
                    None => Err(error),
                };
            }
        };
        self.commit_execution_plan(
            &prepared.plan.order_id,
            &prepared.plan.id,
            &cowork_workflow_contracts::CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
    }

    pub async fn serve(self, parallelism: usize, shutdown: CancellationToken) {
        let mut tasks = JoinSet::new();
        for _ in 0..parallelism {
            let runtime = self.clone();
            let stop = shutdown.clone();
            tasks.spawn(async move {
                loop {
                    if stop.is_cancelled() { break; }
                    match runtime.store.claim_for_worker().await {
                        Ok(Some(run)) => {
                            if run.record.spec.workflow.is_some() {
                                let run_id = run.record.id.clone();
                                if let Err(e) = runtime.execute_claimed_workflow(run, &stop).await {
                                    tracing::warn!(run_id, code=e.code(), "Workflow execution stopped");
                                }
                                continue;
                            }
                            let run_id=run.record.id.clone();
                            let token=run.lease.as_ref().expect("claimed lease").token.clone();
                            if let Err(e)=runtime.execute(&run_id,&token,&stop).await {
                                tracing::warn!(run_id,code=e.code(),"Run execution stopped");
                                let interrupted=stop.is_cancelled() || matches!(e,Error::StaleLease|Error::UncertainEffect|Error::McpSessionUnavailable|Error::NativeKeyUnavailable);
                                let _=runtime.store.mutate(&run_id,Some(&token),|r| {
                                    let interrupted=interrupted||r.checkpoint.in_flight.is_some()||(r.checkpoint.completion_pending&&!matches!(e,Error::BudgetExceeded));
                                    if r.record.state==RunState::CancelRequested && (!interrupted || (r.checkpoint.completion_pending&&r.checkpoint.in_flight.is_none())) {
                                        r.transition(RunState::Canceled)?;
                                    } else if matches!(r.record.state,RunState::Running|RunState::CancelRequested|RunState::WaitingForSnapshot) {
                                        r.transition(if interrupted {RunState::Interrupted}else if matches!(e,Error::BudgetExceeded){RunState::Expired}else{RunState::Failed})?;
                                    } else { return Err(Error::Conflict); }
                                    r.record.error_code=Some(if r.checkpoint.in_flight.is_some(){"uncertain_effect".into()}else if interrupted {"executor_interrupted".into()}else{e.code().into()});
                                    r.lease=None;
                                    Ok(Change::event("run_stopped",json!({"state":r.record.state,"error_code":r.record.error_code})))
                                }).await;
                            }
                        }
                        Ok(None) => tokio::select! { _=stop.cancelled()=>break, _=tokio::time::sleep(Duration::from_millis(100))=>{} },
                        Err(e) => {
                            tracing::error!(code=e.code(),"Could not claim queued run");
                            tokio::select! { _=stop.cancelled()=>break, _=tokio::time::sleep(Duration::from_secs(1))=>{} }
                        }
                    }
                }
            });
        }
        let recovery = self.store.clone();
        let stop = shutdown.clone();
        tasks.spawn(async move {
            loop {
                if let Err(e)=recovery.recover_expired_leases().await { tracing::error!(code=e.code(),"Lease recovery failed"); }
                if let Err(e)=recovery.expire_waiting_runs().await { tracing::error!(code=e.code(),"Waiting budget expiration failed"); }
                tokio::select! { _=stop.cancelled()=>break, _=tokio::time::sleep(Duration::from_secs(2))=>{} }
            }
        });
        while let Some(result) = tasks.join_next().await {
            if result.is_err() {
                tracing::error!("Runtime task panicked; stopping supervisor");
                shutdown.cancel();
            }
        }
    }

    async fn execute(&self, run_id: &str, token: &str, stop: &CancellationToken) -> Result<()> {
        let initial = self.store.stored_run(run_id).await?;
        self.restore_run_workspace(&initial.record).await?;
        // A monotonic deadline prevents a clock adjustment from extending an active execution.
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(remaining_run_millis(&initial.record, now()));
        loop {
            if stop.is_cancelled() {
                return Err(Error::Provider("shutdown".into()));
            }
            let stored = self.store.stored_run(run_id).await?;
            stored.assert_lease(token, now())?;
            if self.ack_cancel(run_id, token, &stored).await? {
                return Ok(());
            }
            if let Some(reference) = &stored.record.spec.environment {
                self.process_environment(&stored.record.spec.project_id, reference)
                    .await?;
            }
            if (!stored.checkpoint.completion_pending
                && stored.record.steps >= stored.record.spec.budgets.max_steps)
                || token_budget(&stored.record).limit_exceeded
                || time_budget_exhausted(&stored.record, now())
                || tokio::time::Instant::now() >= deadline
            {
                self.store
                    .mutate(run_id, Some(token), |r| {
                        r.transition(RunState::Expired)?;
                        r.lease = None;
                        r.record.error_code = Some("budget_exceeded".into());
                        Ok(Change::event(
                            "run_expired",
                            json!({"reason":"budget_exceeded"}),
                        ))
                    })
                    .await?;
                return Ok(());
            }
            if stored.checkpoint.completion_pending {
                return self.finish_workspace(run_id, token, stop, deadline).await;
            }
            self.assert_snapshot_controller(&stored.record.spec).await?;
            if let Some(tool) = stored.checkpoint.pending_tools.first().cloned() {
                if tool.name != "request_input"
                    && stored
                        .record
                        .spec
                        .agent_config
                        .as_ref()
                        .and_then(|c| c.behavior.as_ref())
                        .is_some_and(|b| b.fallback_to_human_on_repeated_failure)
                    && consecutive_tool_failures(&stored.checkpoint.messages) >= 3
                {
                    self.store.mutate(run_id,Some(token),|r|{
                        ensure_running(r)?;ensure_pending(r,&tool)?;r.checkpoint.pending_tools.remove(0);
                        let question="Three consecutive tool operations failed. Review the recorded results and provide corrected inputs or instructions before execution continues.";
                        r.checkpoint.input_request=Some(ToolCall{id:tool.id.clone(),name:"request_input".into(),arguments:json!({"question":question})});
                        r.record.steps+=1;r.transition(RunState::WaitingInput)?;r.lease=None;
                        Ok(Change::event("input_requested",json!({"request_id":tool.id,"question":question,"reason":"repeated_tool_failure","skipped_tool":tool.name})))
                    }).await?;
                    return Ok(());
                }
                let decision = evaluate(
                    &stored.record,
                    &tool,
                    &[],
                    &[],
                    &stored.checkpoint.approvals,
                );
                // Boxed: keeps the document futures out of execute's own state machine.
                if decision != Decision::Deny
                    && cowork_domain::document_tool(&tool.name)
                    && Box::pin(self.reject_document_call(run_id, token, &stored, &tool)).await?
                {
                    continue;
                }
                if decision == Decision::ApprovalRequired {
                    self.store
                        .mutate(run_id, Some(token), |r| {
                            ensure_running(r)?;
                            let approval = Approval {
                                id: id(),
                                run_id: run_id.into(),
                                tool: tool.clone(),
                                target: r.record.spec.executor.clone(),
                                policy_revision: r.record.spec.policy_revision,
                                fingerprint: approval_fingerprint(&r.record, &tool),
                                decision: None,
                            };
                            r.checkpoint.approvals.push(approval.clone());
                            r.transition(RunState::WaitingApproval)?;
                            r.lease = None;
                            Ok(Change::event(
                                "approval_requested",
                                serde_json::to_value(approval).map_err(storage_error)?,
                            ))
                        })
                        .await?;
                    return Ok(());
                }
                if decision == Decision::Deny {
                    self.tool_result(run_id, token, &tool, json!({"error":"policy_denied"}), None)
                        .await?;
                    continue;
                }
                if tool.name == "request_input" {
                    let question = tool.arguments["question"]
                        .as_str()
                        .ok_or_else(|| Error::Validation("Question missing".into()))?;
                    valid_text(question, 4000)?;
                    self.store
                        .mutate(run_id, Some(token), |r| {
                            ensure_running(r)?;
                            ensure_pending(r, &tool)?;
                            r.checkpoint.pending_tools.remove(0);
                            r.checkpoint.input_request = Some(tool.clone());
                            r.record.steps += 1;
                            r.transition(RunState::WaitingInput)?;
                            r.lease = None;
                            Ok(Change::event(
                                "input_requested",
                                json!({"request_id":tool.id,"question":question}),
                            ))
                        })
                        .await?;
                    return Ok(());
                }
                if matches!(
                    tool.name.as_str(),
                    "list_files"
                        | "read_text_file"
                        | "stat_file"
                        | "write_text_file"
                        | "create_directory"
                        | "rename_file"
                        | "remove_file"
                        | "restore_file"
                        | "import_file"
                ) {
                    self.invoke_file_tool(run_id, token, &stored, &tool, stop, deadline)
                        .await?;
                    continue;
                }
                if cowork_domain::document_tool(&tool.name) {
                    Box::pin(
                        self.invoke_document_tool(run_id, token, &stored, &tool, stop, deadline),
                    )
                    .await?;
                    continue;
                }
                if stored
                    .record
                    .spec
                    .mcp_tools
                    .iter()
                    .any(|b| b.alias == tool.name)
                {
                    // Keep the connection/session state out of the worker's
                    // inline future, as for the document engine above. In
                    // debug Windows builds it otherwise exhausts a normal
                    // runtime thread stack before MCP dispatch can complete.
                    let attempt =
                        Box::pin(self.dispatch_configured_mcp_tool(run_id, token, &tool, stop))
                            .await?;
                    match attempt.state {
                        McpAttemptState::Completed { .. } => {
                            // The journal atomically settles the tool and may also
                            // acknowledge cancellation. Never settle twice.
                            if self.store.stored_run(run_id).await?.record.state
                                != RunState::Running
                            {
                                return Ok(());
                            }
                        }
                        _ => return Err(Error::UncertainEffect),
                    }
                    continue;
                }
                if tool.name == "execute_program" {
                    if stored.record.spec.environment.is_some() {
                        self.invoke_process_tool(run_id, token, &tool, stop).await?;
                    } else {
                        self.invoke_native_toolbox(run_id, token, &tool, stop)
                            .await?;
                    }
                    if self.store.stored_run(run_id).await?.record.state != RunState::Running {
                        return Ok(());
                    }
                    continue;
                }
                if toolbox_bridge::is_native_tool(&tool.name) {
                    self.invoke_native_toolbox(run_id, token, &tool, stop)
                        .await?;
                    continue;
                }
                let (result, artifact) = self.invoke_internal_tool(&stored.record, &tool).await?;
                self.tool_result(run_id, token, &tool, result, artifact)
                    .await?;
                continue;
            }
            if stored.record.spec.file_action.is_some() {
                if self.finish_file_action(run_id, token, &stored).await? {
                    return Ok(());
                }
                continue;
            }
            let frozen_provider = stored
                .record
                .spec
                .provider
                .as_ref()
                .ok_or(Error::Forbidden)?;
            let credential = self.personal_provider_credential(frozen_provider).await?;
            // Freeze provider-specific JSON bytes before admission. No HTTP
            // future may rebuild tools/messages after the reservation commits.
            let mut prepared = self
                .prepare_model(&stored.record.spec, &stored.checkpoint.messages)
                .await?;
            let request_hash = prepared.logical_fingerprint().to_owned();
            let wire_hash = prepared.wire_sha256().to_owned();
            let context_measurement = prepared.context_measurement();
            let attempt_id = id();
            self.store
                .admit_prepared_model_attempt(
                    run_id,
                    token,
                    &attempt_id,
                    &request_hash,
                    &wire_hash,
                    |r| {
                        ensure_running(r)?;
                        prepared.assert_binding(&r.record.spec, &r.checkpoint.messages)?;
                        let attempt = reserve_model_tokens(&mut r.record, &request_hash, &attempt_id, now(),prepared.exact_reserved_tokens())?;
                        Ok(Change::event(
                            "model_started",
                            json!({"step":r.record.steps,"attempt":attempt,
                            "prepared_request":{"format":1,"wire_sha256":wire_hash},"context_measurement":context_measurement}),
                        ))
                    },
                )
                .await?;
            // The admission transaction may have waited. Cancellation, lease,
            // exact context and attempt authority are fresh before network I/O.
            let dispatch = self.store.stored_run(run_id).await?;
            dispatch.assert_lease(token, now())?;
            ensure_running(&dispatch)?;
            prepared.assert_binding(&dispatch.record.spec, &dispatch.checkpoint.messages)?;
            self.store.registered_model(&dispatch.record.spec).await?;
            self.assert_snapshot_controller(&dispatch.record.spec)
                .await?;
            if !dispatch.record.model_attempts.iter().any(|attempt| {
                attempt.id == attempt_id
                    && attempt.request_sha256.as_deref() == Some(request_hash.as_str())
                    && attempt.state == ModelAttemptState::InFlight
            }) {
                return Err(Error::UncertainEffect);
            }
            prepared.bind_dispatch_id(run_id, &attempt_id)?;
            if stop.is_cancelled() {
                return Err(Error::Provider("shutdown".into()));
            }
            if time_budget_exhausted(&dispatch.record, now())
                || tokio::time::Instant::now() >= deadline
            {
                return Err(Error::BudgetExceeded);
            }
            let (tx, mut rx) = mpsc::channel(32);
            let money_scope = self
                .store
                .monetary_dispatch_scope(run_id, token, &attempt_id)
                .await?;
            let receive_observer = provider::ProviderReceiveObserver::new(money_scope);
            let future = provider::complete_prepared_observed(
                prepared,
                credential,
                tx,
                receive_observer.clone(),
            );
            tokio::pin!(future);
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            let mut last_heartbeat = now();
            // Every receive/delta/lease exit crosses this one cost-only finally
            // boundary. No `?` inside the block can discard observed usage.
            let receive_result: Result<Option<provider::Reply>> = async {
            let reply = loop {
                tokio::select! {
                    biased;
                    _=stop.cancelled()=>{
                        return Err(Error::Provider("shutdown".into()));
                    },
                    _=tokio::time::sleep_until(deadline)=>{
                        return Err(Error::BudgetExceeded);
                    },
                    _=interval.tick()=>{
                        let current=self.store.stored_run(run_id).await?;
                        if self.ack_cancel(run_id,token,&current).await? {
                            return Ok(None);
                        }
                        if time_budget_exhausted(&current.record,now()) {
                            return Err(Error::BudgetExceeded);
                        }
                        if now()-last_heartbeat>=3000 { self.store.heartbeat(run_id,token).await?; last_heartbeat=now(); }
                    },
                    result=&mut future=>break result?,
                    Some(delta)=rx.recv()=>{
                        self.store.mutate(run_id,Some(token),|r| {
                            ensure_running(r)?;
                            Ok(Change::event("model_delta",json!({"text":delta,"step":r.record.steps})))
                        }).await?;
                    },
                }
            };
            // Drain deltas already sent before completion; the durable final message is authoritative.
            while let Ok(delta) = rx.try_recv() {
                self.store
                    .mutate(run_id, Some(token), |r| {
                        ensure_running(r)?;
                        Ok(Change::event(
                            "model_delta",
                            json!({"text":delta,"step":r.record.steps}),
                        ))
                    })
                    .await?;
            }
            Ok(Some(reply))
            }.await;
            let reply = match receive_result {
                Ok(Some(reply)) => reply,
                Ok(None) => {
                    self.note_provider_uncertainty(run_id, &attempt_id, &receive_observer)
                        .await?;
                    return Ok(());
                }
                Err(error) => {
                    self.note_provider_uncertainty(run_id, &attempt_id, &receive_observer)
                        .await?;
                    return Err(error);
                }
            };
            let planning =
                cowork_domain::hybrid_planning(&stored.record.spec, &stored.checkpoint.messages);
            let finished = reply.tools.is_empty() && !planning;
            let output_error = if finished {
                stored.record.spec.output.as_ref().and_then(|output| {
                    cowork_domain::structured_output::validate_result(output, &reply.content).err()
                })
            } else {
                None
            };
            let tool_error = if planning && !reply.tools.is_empty() {
                Some(Error::Forbidden)
            } else {
                provider::validate_tool_ids(&stored.checkpoint.messages, &reply.tools).err()
            };
            let settled=self.store.settle_received_model_attempt(run_id,token,&attempt_id,reply.received.as_ref(),|r| {
                ensure_running(r)?;
                settle_model(&mut r.record,&attempt_id,&reply.usage,now())?;
                if token_budget(&r.record).limit_exceeded || tool_error.is_some() || output_error.is_some() {
                    let budget=token_budget(&r.record).limit_exceeded;
                    r.transition(if budget{RunState::Expired}else{RunState::Failed})?;
                    r.record.error_code=Some(if budget{"budget_exceeded"}else if output_error.is_some(){"structured_output_mismatch"}else{"provider_unavailable"}.into());
                    r.lease=None;
                    let mut payload=json!({"attempt_id":attempt_id,"usage":reply.usage,"content":reply.content,"output":r.record.spec.output,"error_code":r.record.error_code,"state":r.record.state,"tools_executed":false});
                    if let Some(metadata)=&reply.received_cost_metadata {payload["received_cost_metadata"]=metadata.clone();}
                    return Ok(Change::event("model_response_rejected",payload));
                }
                let calls:Vec<_>=reply.tools.iter().map(|t|json!({"id":t.id,"type":"function",
                    "function":{"name":t.name,"arguments":t.arguments.to_string()}})).collect();
                let mut message=json!({"role":"assistant","content":reply.content});
                if let Some(continuation)=&reply.provider_continuation {message["cowork_provider_continuation"]=continuation.clone();}
                if !calls.is_empty() { message["tool_calls"]=json!(calls); }
                r.checkpoint.messages.push(message);
                if planning {
                    r.checkpoint.messages.push(json!({"role":"system","cowork_phase":cowork_domain::HYBRID_EXECUTION_MARKER,"content":"Planning is complete. Execute the plan now, respecting the selected tools, budgets and required approvals. Report actual results."}));
                }
                r.checkpoint.pending_tools=reply.tools.clone();
                r.checkpoint.last_model_content=reply.content.clone();
                let mut payload=json!({"attempt_id":attempt_id,"content":reply.content,"tool_calls":reply.tools,"usage":reply.usage});
                if let Some(metadata)=&reply.received_cost_metadata {payload["received_cost_metadata"]=metadata.clone();}
                let mut change=Change::event("model_completed",payload);
                if finished && cowork_domain::run_workspaces::retains_workspace(&r.record.spec) {
                    r.checkpoint.completion_pending = true;
                    r.transition(RunState::WaitingForSnapshot)?;
                    change.events.push(("workspace_snapshot_pending".into(),json!({"state":"waiting_for_snapshot"})));
                } else if finished {
                    r.transition(RunState::Completed)?; r.lease=None;
                    change.events.push(("run_completed".into(),json!({"state":"completed"})));
                    change.messages.push(Message { id:id(),order_id:r.record.spec.order_id.clone(),run_id:run_id.into(),
                        role:"assistant".into(),content:reply.content.clone(),created_at:now() });
                }
                Ok(change)
            }).await;
            let settled = match settled {
                Ok(stored) => stored,
                Err(error) => {
                    self.note_provider_uncertainty(run_id, &attempt_id, &receive_observer)
                        .await?;
                    return Err(error);
                }
            };
            if terminal(&settled.record.state) {
                return Ok(());
            }
            if finished && !cowork_domain::run_workspaces::retains_workspace(&stored.record.spec) {
                return Ok(());
            }
        }
    }

    async fn note_provider_uncertainty(
        &self,
        run: &str,
        attempt: &str,
        observer: &provider::ProviderReceiveObserver,
    ) -> Result<()> {
        if let Some(receipt) = observer.interrupted_receipt()? {
            self.store
                .note_received_model_uncertainty(run, attempt, &receipt)
                .await?;
        }
        Ok(())
    }

    async fn ack_cancel(&self, run_id: &str, token: &str, run: &StoredRun) -> Result<bool> {
        if run.record.state != RunState::CancelRequested {
            return Ok(false);
        }
        self.store
            .mutate(run_id, Some(token), |r| {
                r.transition(RunState::Canceled)?;
                r.lease = None;
                Ok(Change::event("run_canceled", json!({"state":"canceled"})))
            })
            .await?;
        Ok(true)
    }

    async fn tool_result(
        &self,
        run_id: &str,
        token: &str,
        tool: &ToolCall,
        result: Value,
        artifact: Option<Artifact>,
    ) -> Result<()> {
        self.store
            .mutate(run_id, Some(token), |r| {
                ensure_running(r)?;
                ensure_pending(r, tool)?;
                r.checkpoint.pending_tools.remove(0);
                r.checkpoint.messages.push(
                    json!({"role":"tool","tool_call_id":tool.id,"content":result.to_string()}),
                );
                r.record.steps += 1;
                let mut change = Change::event(
                    "tool_completed",
                    json!({"tool_call_id":tool.id,"name":tool.name,"result":result}),
                );
                if let Some(a) = &artifact {
                    change.artifacts.push(a.clone());
                }
                Ok(change)
            })
            .await?;
        Ok(())
    }

    /// A tool result whose rendered previews a vision model sees on its next turn.
    pub(crate) async fn tool_result_with_images(
        &self,
        run_id: &str,
        token: &str,
        tool: &ToolCall,
        result: Value,
        images: Vec<Value>,
    ) -> Result<()> {
        if images.is_empty() {
            return self.tool_result(run_id, token, tool, result, None).await;
        }
        self.store
            .mutate(run_id, Some(token), |r| {
                ensure_running(r)?;
                ensure_pending(r, tool)?;
                r.checkpoint.pending_tools.remove(0);
                r.checkpoint.messages.push(
                    json!({"role":"tool","tool_call_id":tool.id,"content":result.to_string(),"cowork_images":images}),
                );
                r.record.steps += 1;
                Ok(Change::event(
                    "tool_completed",
                    json!({"tool_call_id":tool.id,"name":tool.name,"result":result,"images":images}),
                ))
            })
            .await?;
        Ok(())
    }

    async fn invoke_internal_tool(
        &self,
        run: &RunRecord,
        tool: &ToolCall,
    ) -> Result<(Value, Option<Artifact>)> {
        match tool.name.as_str() {
            "create_artifact" => {
                let name = tool.arguments["name"]
                    .as_str()
                    .ok_or_else(|| Error::Validation("Artifact name missing".into()))?;
                let content = tool.arguments["content"]
                    .as_str()
                    .ok_or_else(|| Error::Validation("Artifact content missing".into()))?;
                valid_text(name, 200)?;
                valid_text(content, 1_000_000)?;
                // Name is display metadata, never a filesystem path.
                let artifact = Artifact {
                    id: id(),
                    run_id: run.id.clone(),
                    order_id: run.spec.order_id.clone(),
                    name: name.into(),
                    media_type: "text/plain; charset=utf-8".into(),
                    size: content.len() as u64,
                    sha256: hash(content.as_bytes()),
                    content: content.into(),
                    created_at: now(),
                };
                Ok((
                    json!({"artifact_id":artifact.id,"sha256":artifact.sha256}),
                    Some(artifact),
                ))
            }
            "list_artifacts" => {
                let all = self.store.artifacts(&run.spec.order_id).await?;
                Ok((
                    json!(
                        all.into_iter()
                            .map(
                                |a| json!({"id":a.id,"name":a.name,"sha256":a.sha256,"size":a.size})
                            )
                            .collect::<Vec<_>>()
                    ),
                    None,
                ))
            }
            "read_artifact" => {
                let artifact_id = tool.arguments["id"]
                    .as_str()
                    .ok_or_else(|| Error::Validation("Artifact id missing".into()))?;
                let artifact = self
                    .store
                    .artifacts(&run.spec.order_id)
                    .await?
                    .into_iter()
                    .find(|a| a.id == artifact_id)
                    .ok_or(Error::NotFound)?;
                Ok((
                    json!({"content":artifact.content,"sha256":artifact.sha256}),
                    None,
                ))
            }
            _ => Err(Error::Forbidden),
        }
    }
}
fn ensure_running(run: &StoredRun) -> Result<()> {
    if run.record.state != RunState::Running {
        return Err(Error::Conflict);
    }
    if time_budget_exhausted(&run.record, now()) {
        return Err(Error::BudgetExceeded);
    }
    Ok(())
}
fn spec_from_preflight(input: &CreateRun, order: &Order, flight: &Preflight) -> RunSpec {
    RunSpec {
        input_workspace: None,
        agent_config: input.agent_config.clone(),
        model_binding: flight.model_binding.clone(),
        output: input.output.clone(),
        workflow: None,
        mcp_tools: flight.mcp_tools.clone(),
        environment: flight.environment.clone(),
        order_id: input.order_id.clone(),
        project_id: order.project_id.clone(),
        prompt: input.prompt.clone(),
        provider: Some(flight.provider.clone()),
        file_action: None,
        access_mode: input.access_mode.clone(),
        executor: flight.executor.clone(),
        policy_revision: flight.policy_revision,
        allowed_tools: flight.allowed_tools.clone(),
        budgets: flight.budgets.clone(),
        folders: flight.folders.clone(),
    }
}
fn ensure_pending(run: &StoredRun, tool: &ToolCall) -> Result<()> {
    if run.checkpoint.pending_tools.first() != Some(tool) {
        return Err(Error::Conflict);
    }
    Ok(())
}
