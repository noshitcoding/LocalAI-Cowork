//! Actor-private graph coordination; native owner workflows are never invoked.
use super::*;
use cowork_contracts::RunSpec;
use cowork_domain::{
    canonical_hash,
    workflows::{WorkflowPlan, WorkflowProgress, WorkflowState},
};
use cowork_storage::android_private_tools::{FILE_TOOLS, PrivateToolProgress, folder_id};
use cowork_storage::android_private_workflows::{
    PrivateWorkflowNodeScope, with_private_workflow_node,
};
use std::collections::{BTreeMap, BTreeSet};
use tokio::sync::Mutex;
use uuid::Uuid;
#[path = "android_private_crew.rs"]
mod crew;
pub struct PrivateWorkflowServices {
    store: Store,
    workflow: Value,
    crew: Option<crew::PrivateCrewRuntime>,
    _hold: Arc<dyn Send + Sync>,
}
impl PrivateWorkflowServices {
    pub fn operator_configuration(
        instance: Uuid,
        runner: Option<&Value>,
        configuration: Option<Value>,
        kind: &str,
    ) -> Result<Value> {
        if instance.is_nil() {
            return Err(invalid());
        }
        match kind {
            "workflow" => Ok(
                json!({"format":"android-private-workflow-engine/1","engine":"workflow","instance_id":instance,"protocol":"actor-private-workflow/1","dispatcher_sha256":cowork_domain::hash(include_bytes!("android_private_workflow_worker.rs")),"storage_sha256":cowork_domain::hash(include_bytes!("../../../crates/storage/src/android_private_workflow_execution.rs")),"pipeline_sha256":cowork_domain::hash(include_bytes!("../../../crates/storage/src/android_private_workflow_pipeline.rs")),"compiler_sha256":cowork_domain::hash(include_bytes!("../../../crates/storage/src/android_private_workflow_definition.rs"))}),
            ),
            "crew" => Ok(crew::PrivateCrewRuntime::new(
                instance,
                runner.ok_or(Error::Forbidden)?,
                configuration.ok_or(Error::Forbidden)?,
            )?
            .configuration()
            .clone()),
            _ => Err(invalid()),
        }
    }
    pub async fn from_startup(
        store: Store,
        instance: Uuid,
        runner: Option<Value>,
        configuration: Option<Value>,
        hold: Arc<dyn Send + Sync>,
    ) -> Result<Arc<Self>> {
        if instance.is_nil() {
            return Err(invalid());
        }
        let workflow = Self::operator_configuration(instance, runner.as_ref(), None, "workflow")?;
        store.android_workflow_engine_register(&workflow).await?;
        let crew = match configuration {
            Some(config) => {
                let runtime = crew::PrivateCrewRuntime::new(
                    instance,
                    runner.as_ref().ok_or(Error::Forbidden)?,
                    config,
                )?;
                store
                    .android_workflow_engine_register(runtime.configuration())
                    .await?;
                Some(runtime)
            }
            None => None,
        };
        Ok(Arc::new(Self {
            store,
            workflow,
            crew,
            _hold: hold,
        }))
    }
    pub fn configuration(&self, kind: &str) -> Result<&Value> {
        match kind {
            "workflow" => Ok(&self.workflow),
            "crew" => self
                .crew
                .as_ref()
                .map(|c| c.configuration())
                .ok_or(Error::Forbidden),
            _ => Err(invalid()),
        }
    }
    pub async fn admit(
        &self,
        actor: &cowork_identity::Principal,
        project: Uuid,
        kind: &str,
    ) -> Result<Value> {
        self.store
            .android_workflow_engine_admission(actor, project, self.configuration(kind)?)
            .await
    }
}

pub(super) struct Tools {
    mcp: BTreeMap<String, super::private_mcp_worker::PrivateMcpSession>,
    browsers: BTreeMap<Uuid, super::private_browser::BrowserSession>,
}
impl AndroidDeviceServices {
    pub(crate) async fn private_workflow_admission(
        &self,
        actor: &cowork_identity::Principal,
        project: Uuid,
        kind: &str,
    ) -> Result<Value> {
        self.private_workflows
            .as_ref()
            .ok_or(Error::Forbidden)?
            .admit(actor, project, kind)
            .await
    }
    pub(super) async fn private_workflow(
        self: &Arc<Self>,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        stop: &CancellationToken,
    ) -> Result<()> {
        let p = self
            .store
            .android_private_server_profile(lease.run, lease.lease)
            .await?;
        let input = &lease.value["spec"]["input"];
        if input["private_workflow"].is_null() {
            return Err(invalid());
        }
        let profile = cowork_storage::actor_model_registry::provider_profile(&p)?;
        let (mut mcp, bindings) = self.private_mcp_sessions(lease, stop).await?;
        let initialized=async {
        let mut tools: Vec<String> = FILE_TOOLS.iter().map(|s| s.to_string()).collect();
        tools.push("delegate_agents".into());
        tools.push("read_dependency_result".into());
        tools.extend(bindings.iter().map(|b| b.alias.clone()));
        if let Some(runner) = &self.private_runner
            && input["frozen_private_runner"] == runner.binding() {
                if runner.binding()["programs"]
                    .as_object()
                    .is_some_and(|p| !p.is_empty())
                {
                    tools.push("execute_program".into())
                }
                for name in ["office", "browser", "github"] {
                    if runner.binding()[name] == true
                        && (name != "github" || !input["frozen_github_binding"].is_null())
                    {
                        tools.push(name.into())
                    }
                }
            }
        if let Some(selected) = input["task_config"]["allowed_tools"].as_array() {
            let mut seen = BTreeSet::new();
            for item in selected {
                let item = item.as_str().ok_or_else(invalid)?;
                if !tools.iter().any(|t| t == item) || !seen.insert(item.to_owned()) {
                    return Err(Error::Forbidden);
                }
            }
            tools.retain(|t| seen.contains(t));
        }
        let context = p["model_defaults"]["context_tokens"]
            .as_u64()
            .ok_or_else(invalid)?;
        let prompt = input["prompt"]
            .as_str()
            .or_else(|| input["text"].as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(invalid)?;
        let mut spec:RunSpec=serde_json::from_value(json!({"model_binding":input.get("model_binding"),"output":input.get("output"),"order_id":lease.value["spec"]["thread_id"],"project_id":lease.value["spec"]["project_id"],"prompt":prompt,"provider":profile,"access_mode":"confirm_mutations","executor":"android_private_server_linux","policy_revision":lease.value["spec"]["project"]["revision"],"allowed_tools":tools,"mcp_tools":bindings,"budgets":{"max_steps":p["model_defaults"]["max_steps"].as_u64().unwrap_or(64),"max_seconds":1800,"max_output_tokens":8192u64.min(context/4),"max_total_tokens":context.checked_mul(16).ok_or_else(invalid)?},"folders":[{"id":folder_id(lease.run),"project_id":lease.value["spec"]["project_id"],"name":"Private run workspace","writable":true,"revision":lease.value["spec"]["project"]["revision"].to_string(),"resource_id":lease.run.to_string(),"order_id":lease.value["spec"]["thread_id"]}]})).map_err(|_|invalid())?;
        cowork_runtime::agent_config::apply_agent_run_overrides(&mut spec,input,&p["model_defaults"])?;
        self.store
            .android_private_workflow_initialize(
                lease.run,
                lease.lease,
                &spec,
                self.keys.as_ref(),
                self.key_id,
            )
            .await?;
        Ok::<(),Error>(())
        }.await;
        if let Err(error) = initialized {
            let closed = self.private_mcp_close(lease, &mut mcp).await;
            closed?;
            return Err(error);
        }
        let shared = Arc::new(Mutex::new(Tools {
            mcp,
            browsers: BTreeMap::new(),
        }));
        let mut tasks = tokio::task::JoinSet::new();
        let mut active = BTreeSet::new();
        let child_stop = stop.child_token();
        let mut failed = None;
        let coordinated = async {
        loop {
            if stop.is_cancelled() {
                child_stop.cancel();
                failed = Some(Error::Forbidden);
                break;
            }
            self.store
                .android_private_server_renew(lease.run, lease.lease)
                .await?;
            while let Some(done) = tasks.try_join_next() {
                let (node, outcome) = done.map_err(|_| Error::UncertainEffect)?;
                active.remove(&node);
                if let Err(error) = outcome {
                    child_stop.cancel();
                    failed = Some(error);
                    break;
                }
            }
            if failed.is_some() {
                break;
            }
            let cp = self
                .store
                .android_private_workflow_checkpoint(lease.run, lease.lease, self.keys.as_ref())
                .await?;
            let plan: WorkflowPlan =
                serde_json::from_value(cp["workflow"]["plan"].clone()).map_err(|_| invalid())?;
            let progress: WorkflowProgress =
                serde_json::from_value(cp["workflow"]["progress"].clone())
                    .map_err(|_| invalid())?;
            if active.is_empty()
                && matches!(
                    progress.state(),
                    WorkflowState::Completed | WorkflowState::Failed
                )
            {
                if progress.state() == WorkflowState::Failed {
                    failed = Some(Error::Validation("private_workflow_failed".into()));
                }
                break;
            }
            if active.len() < usize::from(plan.max_parallel) {
                for node in progress.ready_nodes(&plan)? {
                    if active.len() >= usize::from(plan.max_parallel) {
                        break;
                    }
                    if active.contains(&node) {
                        continue;
                    }
                    let scope = self
                        .store
                        .android_private_workflow_claim_node(
                            lease.run,
                            lease.lease,
                            &node,
                            self.keys.as_ref(),
                            self.key_id,
                        )
                        .await?;
                    let service = self.clone();
                    let own_lease = cowork_storage::android_private_server::PrivateServerLease {
                        run: lease.run,
                        lease: lease.lease,
                        value: lease.value.clone(),
                    };
                    let own_stop = child_stop.clone();
                    let own_tools = shared.clone();
                    let id = node.clone();
                    active.insert(node);
                    tasks.spawn(async move {
                        let mut outcome = with_private_workflow_node(scope.clone(),service.private_workflow_node(&own_lease,&scope,&own_stop,own_tools)).await;
                        if outcome.is_err()&&!own_stop.is_cancelled()
                            && matches!(with_private_workflow_node(scope.clone(),service.store.android_private_workflow_fail_node(&scope,service.keys.as_ref(),service.key_id)).await,Ok(true)){outcome=Ok(())}
                        (id, outcome)
                    });
                }
            }
            tokio::select! {_=stop.cancelled()=>{child_stop.cancel();failed=Some(Error::Forbidden);break},_=tokio::time::sleep(Duration::from_millis(100))=>{}}
        }
        Ok::<(),Error>(())
        }.await;
        if let Err(error) = coordinated {
            failed = Some(error);
        }
        if failed.is_some() {
            child_stop.cancel()
        }
        while let Some(outcome) = tasks.join_next().await {
            match outcome {
                Ok((_, Err(error))) => {
                    if failed.is_none() {
                        failed = Some(error)
                    }
                }
                Err(_) => {
                    if failed.is_none() {
                        failed = Some(Error::UncertainEffect)
                    }
                }
                _ => {}
            }
        }
        let mut tools = shared.lock().await;
        // Both original transport owners must finish even when the first close
        // cannot obtain a positive receipt. A dropped waiter never aborts peers.
        let browser_closed = self.private_browser_close(lease, &mut tools.browsers).await;
        let mcp_closed = self.private_mcp_close(lease, &mut tools.mcp).await;
        drop(tools);
        browser_closed?;
        mcp_closed?;
        if let Some(error) = failed {
            return Err(error);
        }
        self.store
            .android_private_workflow_finalize_graph(
                lease.run,
                lease.lease,
                self.keys.as_ref(),
                self.key_id,
            )
            .await?;
        self.store
            .android_private_loop_finish(lease.run, lease.lease, self.keys.as_ref(), self.key_id)
            .await?;
        Ok(())
    }
    async fn private_workflow_node(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        scope: &PrivateWorkflowNodeScope,
        stop: &CancellationToken,
        shared: Arc<Mutex<Tools>>,
    ) -> Result<()> {
        if lease.value["spec"]["input"]["private_workflow"]["engine"] == "crew" {
            return self.private_crew_node(lease, scope, stop, shared).await;
        }
        loop {
            if stop.is_cancelled() {
                return Err(Error::Forbidden);
            }
            self.store
                .android_private_server_renew(lease.run, lease.lease)
                .await?;
            let cp = self
                .store
                .android_private_loop_checkpoint(lease.run, lease.lease, self.keys.as_ref())
                .await?;
            if cp["workflow_yield"] == true {
                return Ok(());
            }
            if cp["pending_tools"]
                .as_array()
                .is_some_and(|a| !a.is_empty())
            {
                // A role's exact tools, arguments, current approval and original effect remain
                // enforced by their existing adapter inside this same validated node scope.
                let completed = self
                    .private_workflow_tool(lease, stop, shared.clone())
                    .await?;
                if !completed {
                    tokio::select! {_=stop.cancelled()=>return Err(Error::Forbidden),_=tokio::time::sleep(Duration::from_millis(200))=>{}}
                }
            } else if cp["last_content"].is_string() && cp["last_turn_has_tools"] != true {
                self.store
                    .android_private_workflow_complete_node(scope, self.keys.as_ref(), self.key_id)
                    .await?;
                return Ok(());
            } else if self
                .store
                .android_private_workflow_pipeline_progress(
                    lease.run,
                    lease.lease,
                    self.keys.as_ref(),
                    self.key_id,
                )
                .await?
            {
                continue;
            } else {
                self.private_model_turn(lease, &cp, stop).await?;
            }
        }
    }
    pub(super) async fn private_workflow_tool(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        stop: &CancellationToken,
        shared: Arc<Mutex<Tools>>,
    ) -> Result<bool> {
        if self
            .store
            .android_private_loop_human_fallback(
                lease.run,
                lease.lease,
                self.keys.as_ref(),
                self.key_id,
            )
            .await?
        {
            return Ok(false);
        }
        let cp = self
            .store
            .android_private_loop_checkpoint(lease.run, lease.lease, self.keys.as_ref())
            .await?;
        let tool: cowork_contracts::ToolCall = serde_json::from_value(
            cp["pending_tools"]
                .as_array()
                .and_then(|a| a.first())
                .ok_or_else(invalid)?
                .clone(),
        )
        .map_err(|_| invalid())?;
        let mut tools = shared.lock().await;
        if stop.is_cancelled() {
            return Err(Error::Forbidden);
        }
        let completed = if tool.name == "read_dependency_result" {
            self.store
                .android_private_workflow_read_dependency(
                    lease.run,
                    lease.lease,
                    self.keys.as_ref(),
                    self.key_id,
                )
                .await?;
            true
        } else if tool.name == "delegate_agents" {
            self.store
                .android_private_workflow_delegate(
                    lease.run,
                    lease.lease,
                    self.keys.as_ref(),
                    self.key_id,
                )
                .await?;
            true
        } else if tool.name == "browser" {
            self.private_browser_tool(lease, stop, &mut tools.browsers)
                .await?
        } else if tool.name == "github" {
            self.private_github_tool(lease, stop).await?
        } else if matches!(tool.name.as_str(), "execute_program" | "office") {
            self.private_program_tool(lease, stop).await?
        } else if cp["spec"]["mcp_tools"]
            .as_array()
            .is_some_and(|a| a.iter().any(|b| b["alias"] == tool.name))
        {
            self.private_mcp_tool(lease, &cp, &mut tools.mcp, stop)
                .await?
        } else {
            matches!(
                self.store
                    .android_private_loop_tool(
                        lease.run,
                        lease.lease,
                        self.keys.as_ref(),
                        self.key_id
                    )
                    .await?,
                PrivateToolProgress::Completed(_)
            )
        };
        drop(tools);
        Ok(completed)
    }
}
