//! Snapshot-bound owner admission. A requested process/connector is never
//! silently executed through the explicitly process-free broker destination.
use super::*;
use cowork_domain::{
    execution_inputs::CapturedExecutionInput,
    run_workspaces::{BROKER_TOOLS, RunWorkspaceIntent, broker_scope},
};
use cowork_storage::{StoredExecutionPlanV2, StoredExecutionRequestV2};
use cowork_workflow_contracts::{
    CommitExecutionPlan, ExecutionBoundary, ExecutionDestinationV2, ExecutionNetworkAccess,
    ExecutionPlanV2, ExecutionPreflightV2, ExecutionProcessAccess, OwnerExecutionTargetV2,
    PrepareOwnerExecutionPlanV2, PreparedExecutionPlanV2, RunPreflight, RunRequest,
};
mod documents;
mod import;
pub(crate) mod managed;
mod restore;
mod workflow_inputs;

struct Resolved {
    source: RunSpec,
    spec: RunSpec,
    flight: ExecutionPreflightV2,
    capture: CapturedExecutionInput,
    workspace: RunWorkspaceIntent,
    origin: String,
    destination: ExecutionDestinationV2,
}

impl Runtime {
    pub async fn retire_execution_request_v2(
        &self,
        request: &PrepareOwnerExecutionPlanV2,
    ) -> Result<cowork_workflow_contracts::RetiredExecutionRequest> {
        self.store.retire_execution_request_v2(request).await
    }
    pub(super) async fn assert_snapshot_controller(&self, spec: &RunSpec) -> Result<()> {
        if spec.input_workspace.is_none() {
            return Ok(());
        }
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let origin = tokio::task::spawn_blocking(move || files.local_controller_origin())
            .await
            .map_err(storage_error)??;
        self.store
            .assert_run_workspace_controller(spec, &origin)
            .await
    }
    async fn resolve_execution_v2(
        &self,
        input: &PrepareOwnerExecutionPlanV2,
        namespace: &str,
        nonce: &str,
    ) -> Result<Resolved> {
        let (order, environment, native, mcp, filter) = match &input.request {
            RunRequest::Single(r) => (
                &r.order_id,
                &r.environment,
                &r.native_tools,
                &r.mcp_tools,
                r.agent_config.as_ref().and_then(|c| c.tool_filter.as_ref()),
            ),
            RunRequest::Workflow(r) => (
                &r.order_id,
                &r.environment,
                &r.native_tools,
                &r.mcp_tools,
                r.agent_config.as_ref().and_then(|c| c.tool_filter.as_ref()),
            ),
        };
        let managed = matches!(input.target, OwnerExecutionTargetV2::RegisteredRunner {});
        let documents = matches!(input.target, OwnerExecutionTargetV2::DocumentEngine {});
        if managed && !matches!(input.request, RunRequest::Single(_)) {
            return Err(Error::Validation(
                "managed_workflow_input_adapter_required".into(),
            ));
        }
        if (!managed && environment.is_some())
            || (managed && environment.is_none())
            || !native.is_empty()
            || !mcp.is_empty()
            || filter.is_some_and(|tools| {
                tools.iter().any(|t| {
                    !(BROKER_TOOLS.contains(&t.as_str())
                        || managed && t == "execute_program"
                        || documents && cowork_domain::document_tool(t))
                })
            })
        {
            return Err(Error::Validation(
                "snapshot_process_or_connector_adapter_required".into(),
            ));
        }
        let stored = self
            .store
            .execution_input(order, &input.input.snapshot_id)
            .await?;
        if stored.retired
            || stored.capture.sha256 != input.input.sha256
            || stored.capture.snapshot.manifest.sha256 != input.input.manifest_sha256
        {
            return Err(Error::ExecutionPlanChanged);
        }
        let files = self
            .files
            .clone()
            .ok_or(Error::Validation("snapshot_file_broker_required".into()))?;
        let origin = tokio::task::spawn_blocking(move || files.local_controller_origin())
            .await
            .map_err(storage_error)??;
        let controller = self.store.broker_controller(&origin).await?;
        let resolved_request = self.resolve_workflow_input_source(&input.request).await?;
        if !managed && !documents {
            self.reject_broker_document_workflow(&resolved_request)
                .await?;
        }
        if documents {
            self.require_document_tools()?;
        }
        let (mut flight, mut source) = self
            .resolve_execution_intent_target(&resolved_request, documents)
            .await?;
        // Intersect automatically available document tools with the explicitly
        // selected broker destination. Explicit requests above are rejected.
        if !documents {
            source
                .allowed_tools
                .retain(|t| !cowork_domain::document_tool(t));
        }
        if !(if managed {
            cowork_domain::run_workspaces::managed_process_scope(&source)
        } else if documents {
            cowork_storage::document_execution_scope(&source)
        } else {
            broker_scope(&source)
        }) {
            return Err(Error::Validation(
                "snapshot_process_or_connector_adapter_required".into(),
            ));
        }
        let destination = if managed {
            self.managed_execution_destination(&source, controller)
                .await?
        } else if documents {
            self.document_execution_destination(&source, controller)?
        } else {
            ExecutionDestinationV2::BrokerController { controller }
        };
        let workspace = RunWorkspaceIntent::for_spec(&stored.capture, &source, namespace, nonce)?;
        let mut spec = source.clone();
        spec.input_workspace = Some(workspace.binding.clone());
        spec.folders = workspace.folders();
        match &mut flight {
            RunPreflight::Single(f) => {
                f.folders = spec.folders.clone();
                f.allowed_tools = spec.allowed_tools.clone();
            }
            RunPreflight::Workflow(f) => {
                if let RunRequest::Workflow(request) = &input.request {
                    f.input_source = request.input_source.clone();
                }
                f.allowed_tools = spec.allowed_tools.clone();
            }
        }
        Ok(Resolved {
            source,
            spec,
            flight: ExecutionPreflightV2::Owner { review: flight },
            capture: stored.capture,
            workspace,
            origin,
            destination,
        })
    }

    pub async fn prepare_execution_plan_v2(
        &self,
        request: &PrepareOwnerExecutionPlanV2,
    ) -> Result<PreparedExecutionPlanV2> {
        if let Some(saved) = self.store.existing_execution_plan_v2(request).await? {
            return self.store.execution_plan_v2_view(&saved).await;
        }
        let namespace = id();
        let nonce = id();
        let resolved = self
            .resolve_execution_v2(request, &namespace, &nonce)
            .await?;
        self.retain_resolved_execution_v2(request.clone().into(), resolved)
            .await
    }

    async fn retain_resolved_execution_v2(
        &self,
        request: StoredExecutionRequestV2,
        resolved: Resolved,
    ) -> Result<PreparedExecutionPlanV2> {
        let at = now();
        let key = request.fields().1.to_owned();
        let managed = matches!(
            resolved.destination,
            ExecutionDestinationV2::ManagedRunner { .. }
        );
        let documents = matches!(
            resolved.destination,
            ExecutionDestinationV2::DocumentEngine { .. }
        );
        let boundary = ExecutionBoundary {
            executor: resolved.spec.executor.clone(),
            process_access: if managed {
                ExecutionProcessAccess::QualifiedEnvironment
            } else if documents {
                ExecutionProcessAccess::UserRights
            } else {
                ExecutionProcessAccess::BrokerOnly
            },
            process_network: if managed {
                ExecutionNetworkAccess::Denied
            } else if documents {
                ExecutionNetworkAccess::UserRights
            } else {
                ExecutionNetworkAccess::NoProcess
            },
            environment: resolved.spec.environment.clone(),
            folders: resolved.spec.folders.clone(),
            connector_aliases: vec![],
            input_snapshot_bound: true,
        };
        let plan = ExecutionPlanV2 {
            format_version: 2,
            id: id(),
            actor: ExecutionActor::InstallationOwner {},
            project_id: resolved.spec.project_id.clone(),
            order_id: resolved.spec.order_id.clone(),
            idempotency_key: key,
            request_sha256: canonical_hash(&request),
            spec_sha256: canonical_hash(&resolved.spec),
            created_at: at,
            expires_at: at + 300_000,
            target: resolved.destination,
            input: resolved.capture.snapshot.clone(),
            workspace: resolved.workspace.binding.clone(),
            preflight: resolved.flight,
            boundary,
        };
        let saved = StoredExecutionPlanV2 {
            sha256: canonical_hash(&plan),
            plan,
            request,
            source_spec: resolved.source,
            source_origins: resolved.capture.origins,
            spec: resolved.spec,
            workspace: resolved.workspace,
        };
        let stored = self.store.retain_execution_plan_v2(&saved).await?;
        self.store.execution_plan_v2_view(&stored).await
    }

    pub async fn inspect_execution_plan_v2(
        &self,
        order: &str,
        plan: &str,
    ) -> Result<PreparedExecutionPlanV2> {
        self.store.order(order).await?;
        let saved = self.store.execution_plan_v2(order, plan).await?;
        self.store.execution_plan_v2_view(&saved).await
    }

    pub async fn commit_execution_plan_v2(
        &self,
        order: &str,
        plan: &str,
        input: &CommitExecutionPlan,
    ) -> Result<RunRecord> {
        let saved = self.store.execution_plan_v2(order, plan).await?;
        if saved.sha256 != input.sha256 {
            return Err(Error::ExecutionPlanChanged);
        }
        if let Some(receipt) = self.store.execution_plan_v2_receipt(&saved).await? {
            return Ok(receipt);
        }
        let accepted = async {
            if now() < saved.plan.created_at || now() >= saved.plan.expires_at {
                return Err(Error::ExecutionPlanExpired);
            }
            let current = match &saved.request {
                StoredExecutionRequestV2::Owner(request) => {
                    self.resolve_execution_v2(
                        request,
                        &saved.workspace.binding.id,
                        &saved.workspace.nonce,
                    )
                    .await?
                }
                StoredExecutionRequestV2::Restore(request) => {
                    self.resolve_restore_execution_v2(
                        request,
                        &saved.workspace.binding.id,
                        &saved.workspace.nonce,
                    )
                    .await?
                }
                StoredExecutionRequestV2::Import(request) => {
                    self.resolve_import_execution_v2(
                        request,
                        &saved.workspace.binding.id,
                        &saved.workspace.nonce,
                    )
                    .await?
                }
            };
            if canonical_hash(&current.destination) != canonical_hash(&saved.plan.target)
                || canonical_hash(&current.source) != canonical_hash(&saved.source_spec)
                || canonical_hash(&current.spec) != saved.plan.spec_sha256
                || canonical_hash(&current.flight) != canonical_hash(&saved.plan.preflight)
                || canonical_hash(&current.capture) != canonical_hash(&saved.capture())
            {
                return Err(Error::ExecutionPlanChanged);
            }
            let configured = self.providers.read().await;
            let profiles = match &current.flight {
                ExecutionPreflightV2::Owner {
                    review: RunPreflight::Single(f),
                } => vec![&f.provider],
                ExecutionPreflightV2::Owner {
                    review: RunPreflight::Workflow(f),
                } => f.roles.iter().map(|r| &r.provider).collect(),
                ExecutionPreflightV2::Restore { .. } | ExecutionPreflightV2::Import { .. } => {
                    vec![]
                }
                _ => return Err(Error::ExecutionPlanChanged),
            };
            for profile in profiles {
                if !configured
                    .iter()
                    .any(|p| canonical_hash(&p.profile) == canonical_hash(profile))
                {
                    return Err(Error::ExecutionPlanChanged);
                }
            }
            let waiting_until = tokio::time::Instant::now() + Duration::from_secs(10);
            let prepared = loop {
                if let Some(receipt) = self.store.execution_plan_v2_receipt(&saved).await? {
                    return Ok(receipt);
                }
                let permit = tokio::time::timeout_at(
                    waiting_until,
                    self.upload_transfers.clone().acquire_owned(),
                )
                .await
                .map_err(|_| Error::TransferBusy)?
                .map_err(|_| Error::TransferBusy)?;
                let files = self.files.clone().ok_or(Error::Forbidden)?;
                let intent = saved.workspace.clone();
                let capture = current.capture.clone();
                let copied = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    files.prepare_run_workspace(&intent, &capture)
                })
                .await
                .map_err(storage_error)?;
                match copied {
                    Ok(prepared) => break prepared,
                    Err(Error::WorkspaceBusy) if tokio::time::Instant::now() < waiting_until => {
                        // Only wait for private-copy custody or the original
                        // commit. No model/tool dispatch is retried here.
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    Err(error) => return Err(error),
                }
            };
            let files = self.files.clone().ok_or(Error::Forbidden)?;
            let fresh_origin = tokio::task::spawn_blocking(move || files.local_controller_origin())
                .await
                .map_err(storage_error)??;
            if fresh_origin != current.origin {
                return Err(Error::ExecutionPlanChanged);
            }
            let result = self
                .store
                .create_execution_plan_v2_run(
                    &saved,
                    &fresh_origin,
                    &serde_json::to_value(&prepared.record).map_err(storage_error)?,
                )
                .await;
            drop(prepared);
            drop(configured);
            result
        }
        .await;
        if accepted.is_err()
            && let Some(receipt) = self.store.execution_plan_v2_receipt(&saved).await?
        {
            return Ok(receipt);
        }
        accepted
    }

    pub async fn retire_execution_plan_v2(
        &self,
        order: &str,
        plan: &str,
        input: &CommitExecutionPlan,
    ) -> Result<PreparedExecutionPlanV2> {
        let saved = self.store.execution_plan_v2(order, plan).await?;
        if saved.sha256 != input.sha256 {
            return Err(Error::ExecutionPlanChanged);
        }
        self.store.retire_execution_plan_v2(&saved).await?;
        self.store.execution_plan_v2_view(&saved).await
    }
}
