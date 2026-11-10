//! Public workflow selection delegates to the shared durable native runtime.
//! Browser data never supplies a RunSpec, device grant, or execution permit.
use super::*;
use cowork_workflow_contracts::{
    RunPreflight, RunRequest, WorkflowModelView, WorkflowPreflight, WorkflowRunRequest,
};
use std::collections::BTreeMap;

const PRODUCT_WORKFLOW_PARALLELISM: u16 = 4;
const PRODUCT_WORKFLOW_TOOLS: [&str; 4] = [
    "create_artifact",
    "list_artifacts",
    "read_artifact",
    "request_input",
];

impl Runtime {
    /// Existing single-agent requests retain their original wire shape and path.
    pub async fn preflight_request(&self, input: &RunRequest) -> Result<RunPreflight> {
        match input {
            RunRequest::Single(input) => self.preflight(input).await.map(RunPreflight::Single),
            RunRequest::Workflow(input) => self
                .preflight_workflow(input)
                .await
                .map(RunPreflight::Workflow),
        }
    }

    pub async fn create_run_request(&self, input: &RunRequest) -> Result<RunRecord> {
        match input {
            RunRequest::Single(input) => self.create_run(input).await,
            RunRequest::Workflow(input) => self.create_product_workflow_run(input).await,
        }
    }

    pub async fn preflight_workflow(
        &self,
        input: &WorkflowRunRequest,
    ) -> Result<WorkflowPreflight> {
        self.preflight_workflow_target(input, false).await
    }

    pub(super) async fn preflight_workflow_target(
        &self,
        input: &WorkflowRunRequest,
        document_target: bool,
    ) -> Result<WorkflowPreflight> {
        if input.input_source.is_some() {
            return Err(Error::Validation(
                "workflow_input_source_requires_plan_v2".into(),
            ));
        }
        valid_text(&input.prompt, 64_000)?;
        validate_budgets(&input.budgets)?;
        self.store.native_chat_available(&input.order_id).await?;
        let order = self.store.order(&input.order_id).await?;
        if order.revision != input.base_revision {
            return Err(Error::Conflict);
        }
        self.selected_native_tools(&input.native_tools)?;
        let spec = self.resolve_workflow_scope(input, &order).await?;
        if !document_target {
            self.reject_broker_document_workflow(&RunRequest::Workflow(input.clone()))
                .await?;
        }
        // Compilation reads authoritative versions/providers, checks capability
        // support and intersects the definition with the trusted product scope.
        let plan = self
            .store
            .product_preflight_compilation(input, &spec)
            .await?;
        plan.validate_wait_inputs(input.inputs.as_deref())?;
        if plan.max_parallel > PRODUCT_WORKFLOW_PARALLELISM {
            return Err(Error::Forbidden);
        }
        plan.validate_native_step_rights()?;
        let binding = WorkflowBinding {
            inputs: input.inputs.clone(),
            workflow_id: input.workflow_id.clone(),
            version_id: input.version_id.clone(),
            definition_sha256: plan.source_sha256.clone(),
            plan_sha256: plan.sha256.clone(),
        };
        if input
            .expected_binding
            .as_ref()
            .is_some_and(|expected| expected != &binding)
        {
            return Err(Error::Conflict);
        }
        // Every pinned profile is checked, including roles whose node does not
        // run initially. No profile or endpoint is selected as a fallback.
        for profile in &plan.profiles {
            let configured = self.providers.read().await;
            let current = configured
                .iter()
                .find(|p| p.profile.id == profile.id)
                .ok_or_else(|| Error::Provider("profile_missing".into()))?;
            if canonical_hash(&current.profile) != canonical_hash(profile) {
                return Err(Error::Provider("profile_changed".into()));
            }
            drop(configured);
            let mut provider_spec = spec.clone();
            provider_spec.provider = Some(profile.clone());
            let credential = self.workflow_credential(&provider_spec).await?;
            self.probe_configured_provider(profile, credential).await?;
        }
        let mut tools_by_node = BTreeMap::new();
        for (id, node) in &plan.nodes {
            if node.role_id.is_none() {
                continue;
            }
            let mut node_spec = spec.clone();
            node_spec.agent_config = cowork_domain::merge_agent_values(
                spec.agent_config.as_ref(),
                plan.roles[node.role()?].agent_config.as_ref(),
            );
            node_spec.provider = Some(plan.roles[node.role()?].provider.clone());
            node_spec.budgets = node.budgets.clone();
            node_spec
                .allowed_tools
                .retain(|tool| node.rights.tools.contains(tool));
            tools_by_node.insert(id.clone(), provider::tool_catalog(&node_spec));
        }
        // Reuse the storage seed builder without claiming nodes, persisting
        // turns, creating workspaces or contacting a completion endpoint.
        let contexts = self
            .store
            .preview_product_workflow_contexts(input, &spec, &plan, &tools_by_node)
            .await?;
        for context in contexts {
            let node = context
                .messages
                .last()
                .and_then(|message| message["content"].as_str())
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                .and_then(|value| value["workflow_step"].as_str().map(str::to_owned));
            let deterministic = node.as_ref().is_some_and(|node| {
                plan.definition
                    .steps
                    .iter()
                    .any(|step| &step.id == node && step.execution.is_some())
            });
            if !deterministic {
                self.prepare_model(&context.spec, &context.messages).await?;
            }
            if (if deterministic {
                cowork_domain::workflow_preparation_fingerprint(
                    &context.spec,
                    &context.messages,
                    &context.tools,
                )?
            } else {
                provider::request_fingerprint(&context.spec, &context.messages)?
            }) != context.request_sha256
            {
                return Err(Error::Conflict);
            }
        }
        let version = self
            .store
            .workflow_version(&order.project_id, &input.workflow_id, &input.version_id)
            .await?;
        if version.reference.sha256 != plan.source_sha256 {
            return Err(Error::Conflict);
        }
        Ok(WorkflowPreflight {
            input_source: None,
            ready: true,
            workflow: binding,
            name: plan.definition.name.clone(),
            version_revision: version.reference.revision,
            executor: spec.executor,
            policy_revision: spec.policy_revision,
            access_mode: spec.access_mode,
            budgets: plan.budgets.clone(),
            allowed_tools: spec.allowed_tools,
            roles: plan
                .roles
                .values()
                .map(|role| WorkflowModelView {
                    role_id: role.id.clone(),
                    provider: role.provider.clone(),
                })
                .collect(),
            max_parallel: plan.max_parallel,
        })
    }

    pub async fn create_product_workflow_run(
        &self,
        input: &WorkflowRunRequest,
    ) -> Result<RunRecord> {
        if let Some(existing) = self.store.existing_product_workflow_request(input).await? {
            if let Some(plan) = self
                .store
                .existing_execution_plan(&RunRequest::Workflow(input.clone()))
                .await?
                && let Some(receipt) = self.store.execution_plan_run(&plan).await?
            {
                return Ok(receipt);
            }
            return Ok(existing);
        }
        let prepared = match self
            .prepare_execution_plan(&RunRequest::Workflow(input.clone()))
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                return match self.store.existing_product_workflow_request(input).await? {
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
    pub(super) async fn resolve_workflow_scope(
        &self,
        input: &WorkflowRunRequest,
        order: &Order,
    ) -> Result<RunSpec> {
        let mut spec = product_workflow_spec(input, order);
        if !input.folders.is_empty() {
            self.files
                .as_ref()
                .ok_or(Error::Forbidden)?
                .validate_grants(&order.project_id, &input.folders)?;
        }
        // A snapshot review has already prepared the app-owned order workspace.
        // Include that existing default even for broker-only runs, without
        // turning a plain preflight into workspace creation. Caller-supplied
        // grants remain restricted to registered project roots.
        if self.files.is_some()
            && (input.environment.is_some() || self.store.workspace(&order.id).await?.is_some())
        {
            let workspace = self.workspace(&order.id).await?.folder;
            if !spec.folders.iter().any(|f| f.id == workspace.id) {
                spec.folders.push(workspace);
            }
        }
        if !spec.folders.is_empty() {
            spec.allowed_tools
                .extend(["list_files", "read_text_file", "stat_file"].map(str::to_owned));
            if spec.folders.iter().any(|f| f.writable) {
                spec.allowed_tools.extend(
                    [
                        "write_text_file",
                        "create_directory",
                        "rename_file",
                        "remove_file",
                        "restore_file",
                    ]
                    .map(str::to_owned),
                );
            }
        }
        spec.mcp_tools = self
            .store
            .select_mcp_tools(&order.project_id, &input.mcp_tools)
            .await?;
        if !spec.mcp_tools.is_empty() {
            self.validate_owner_mcp_bindings(&order.project_id, &spec.mcp_tools)
                .await?;
            spec.allowed_tools
                .extend(spec.mcp_tools.iter().map(|b| b.alias.clone()));
        }
        if let Some(reference) = &input.environment {
            if !input.native_tools.is_empty()
                || !spec
                    .folders
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
            spec.executor = observation.runner_id;
            if !spec.allowed_tools.iter().any(|t| t == "execute_program") {
                spec.allowed_tools.push("execute_program".into());
            }
        }
        let version = self
            .store
            .workflow_version(&order.project_id, &input.workflow_id, &input.version_id)
            .await?;
        let uses_documents = workflow_documents::definition_uses_documents(&version.definition);
        if uses_documents {
            if spec.environment.is_some() {
                return Err(Error::Validation(
                    "workflow_document_target_unavailable".into(),
                ));
            }
            self.require_document_tools()?;
        }
        if workflow_documents::definition_uses_ocr(&version.definition)
            && self
                .toolbox
                .as_ref()
                .is_none_or(|toolbox| !toolbox.availability().ocr_available)
        {
            return Err(Error::Validation(
                "workflow_document_ocr_unavailable".into(),
            ));
        }
        if uses_documents
            && spec.environment.is_none()
            && !spec.folders.is_empty()
            && self.document_tools_ready()
        {
            spec.allowed_tools.push("document_read".into());
            if spec.folders.iter().any(|folder| folder.writable) {
                spec.allowed_tools
                    .extend(["document_write".into(), "image_edit".into()]);
            }
        }
        if version
            .definition
            .roles
            .iter()
            .any(|r| !r.delegate_to.is_empty())
        {
            spec.allowed_tools.push("delegate_agents".into());
        }
        self.freeze_workspace_agent_context(&mut spec, order)
            .await?;
        if let Some(filter) = spec
            .agent_config
            .as_ref()
            .and_then(|c| c.tool_filter.as_ref())
        {
            if filter.iter().any(|tool| !spec.allowed_tools.contains(tool)) {
                return Err(Error::Validation(
                    "A selected workflow agent tool is unavailable".into(),
                ));
            }
            spec.allowed_tools.retain(|tool| filter.contains(tool));
            spec.mcp_tools
                .retain(|binding| filter.contains(&binding.alias));
        }
        if matches!(spec.access_mode, AccessMode::ReadOnly) {
            spec.allowed_tools
                .retain(|tool| cowork_domain::read_only_tool(tool));
            spec.mcp_tools.clear();
            for folder in &mut spec.folders {
                folder.writable = false;
            }
        }
        if spec.environment.is_none() {
            spec.executor = if spec.allowed_tools.iter().any(|tool| {
                matches!(
                    tool.as_str(),
                    "execute_program" | "browser" | "office" | "github"
                )
            }) {
                "owner-native-toolbox-v1"
            } else {
                "local-v2"
            }
            .into();
        }
        if spec.agent_config.as_ref().and_then(|c| c.execution_mode)
            == Some(AgentExecutionMode::Plan)
        {
            spec.allowed_tools.clear();
            spec.mcp_tools.clear();
        }
        Ok(spec)
    }
}

pub(super) fn product_workflow_spec(input: &WorkflowRunRequest, order: &Order) -> RunSpec {
    RunSpec {
        input_workspace: None,
        agent_config: input.agent_config.clone(),
        model_binding: None,
        output: None,
        workflow: None,
        mcp_tools: vec![],
        environment: input.environment.clone(),
        order_id: input.order_id.clone(),
        project_id: order.project_id.clone(),
        prompt: input.prompt.clone(),
        provider: None,
        file_action: None,
        access_mode: input.access_mode.clone(),
        executor: if input.native_tools.is_empty() {
            "local-v2"
        } else {
            "owner-native-toolbox-v1"
        }
        .into(),
        policy_revision: 1,
        allowed_tools: PRODUCT_WORKFLOW_TOOLS
            .iter()
            .map(|tool| (*tool).into())
            .chain(input.native_tools.iter().cloned())
            .collect(),
        // Preserve the requested scope during compilation. Storage freezes its
        // effective intersection in the accepted run, never in the request hash.
        budgets: input.budgets.clone(),
        folders: input.folders.clone(),
    }
}
