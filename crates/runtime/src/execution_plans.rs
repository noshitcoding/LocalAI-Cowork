//! Review resolves trusted scope; commit compares it before atomic durable admission.
use super::*;
use cowork_storage::StoredExecutionPlan;
use cowork_workflow_contracts::{
    CommitExecutionPlan, ExecutionBoundary, ExecutionNetworkAccess, ExecutionPlan,
    ExecutionProcessAccess, PreparedExecutionPlan, RunPreflight, RunRequest,
};

impl Runtime {
    pub(super) async fn resolve_execution_intent(
        &self,
        request: &RunRequest,
    ) -> Result<(RunPreflight, RunSpec)> {
        self.resolve_execution_intent_target(request, false).await
    }

    pub(super) async fn resolve_execution_intent_target(
        &self,
        request: &RunRequest,
        document_target: bool,
    ) -> Result<(RunPreflight, RunSpec)> {
        let flight = match request {
            RunRequest::Workflow(input) if document_target => {
                RunPreflight::Workflow(self.preflight_workflow_target(input, true).await?)
            }
            _ => self.preflight_request(request).await?,
        };
        let spec = match (request, &flight) {
            (RunRequest::Single(input), RunPreflight::Single(preflight)) => {
                let order = self.store.order(&input.order_id).await?;
                self.configured_run_spec(input, &order, preflight).await?
            }
            (RunRequest::Workflow(input), RunPreflight::Workflow(_)) => {
                let order = self.store.order(&input.order_id).await?;
                self.resolve_workflow_scope(input, &order).await?
            }
            _ => return Err(Error::ExecutionPlanChanged),
        };
        Ok((flight, spec))
    }

    async fn execution_boundary(&self, spec: &RunSpec) -> Result<ExecutionBoundary> {
        let (process_access, process_network) = if let Some(reference) = &spec.environment {
            let record = self
                .store
                .environment(&spec.project_id, &reference.environment_id)
                .await?;
            let version = record
                .versions
                .iter()
                .find(|version| version.id == reference.version_id)
                .ok_or(Error::NotFound)?;
            let network = match &version.definition.network {
                None | Some(cowork_domain::environments::EnvironmentNetwork::Denied) => {
                    ExecutionNetworkAccess::Denied
                }
                Some(cowork_domain::environments::EnvironmentNetwork::WebProxy { policy_id }) => {
                    ExecutionNetworkAccess::WebProxy {
                        policy_id: policy_id.clone(),
                    }
                }
            };
            (ExecutionProcessAccess::QualifiedEnvironment, network)
        } else if spec.executor == "owner-native-toolbox-v1" {
            (
                ExecutionProcessAccess::UserRights,
                ExecutionNetworkAccess::UserRights,
            )
        } else {
            (
                ExecutionProcessAccess::BrokerOnly,
                ExecutionNetworkAccess::NoProcess,
            )
        };
        Ok(ExecutionBoundary {
            executor: spec.executor.clone(),
            process_access,
            process_network,
            environment: spec.environment.clone(),
            folders: spec.folders.clone(),
            connector_aliases: spec
                .mcp_tools
                .iter()
                .map(|binding| binding.alias.clone())
                .collect(),
            input_snapshot_bound: false,
        })
    }

    pub async fn prepare_execution_plan(
        &self,
        request: &RunRequest,
    ) -> Result<PreparedExecutionPlan> {
        if let Some(saved) = self.store.existing_execution_plan(request).await? {
            return self.execution_plan_view(&saved).await;
        }
        let (flight, spec) = self.resolve_execution_intent(request).await?;
        let boundary = self.execution_boundary(&spec).await?;
        let at = now();
        let key = match request {
            RunRequest::Single(input) => input.idempotency_key.clone(),
            RunRequest::Workflow(input) => input.idempotency_key.clone(),
        };
        let plan = ExecutionPlan {
            format_version: 1,
            id: id(),
            actor: "installation-owner".into(),
            project_id: spec.project_id.clone(),
            order_id: spec.order_id.clone(),
            idempotency_key: key,
            request_sha256: canonical_hash(request),
            spec_sha256: canonical_hash(&spec),
            created_at: at,
            expires_at: at.checked_add(300_000).ok_or(Error::ExecutionPlanExpired)?,
            preflight: flight,
            boundary,
        };
        let prepared = StoredExecutionPlan {
            sha256: canonical_hash(&plan),
            plan,
            request: request.clone(),
            spec,
        };
        let target = if prepared.spec.environment.is_some() {
            Some(
                self.process_runner
                    .as_ref()
                    .and_then(|runner| runner.registered_target())
                    .ok_or(Error::ExecutionPlanChanged)?,
            )
        } else {
            None
        };
        let saved = match self
            .store
            .retain_execution_plan_with_target(&prepared, target)
            .await
        {
            Ok(saved) => saved,
            Err(error) => match self.store.existing_execution_plan(request).await? {
                Some(saved) => saved,
                None => return Err(error),
            },
        };
        self.execution_plan_view(&saved).await
    }

    async fn execution_plan_view(
        &self,
        saved: &StoredExecutionPlan,
    ) -> Result<PreparedExecutionPlan> {
        let receipt = self.store.execution_plan_run(saved).await?;
        let mut view = saved.view(receipt.map(|run| run.id));
        view.retired = self.store.execution_plan_retired(saved).await?;
        Ok(view)
    }

    pub async fn inspect_execution_plan(
        &self,
        order: &str,
        plan: &str,
    ) -> Result<PreparedExecutionPlan> {
        self.store.order(order).await?;
        let saved = self.store.execution_plan(order, plan).await?;
        self.execution_plan_view(&saved).await
    }

    pub async fn commit_execution_plan(
        &self,
        order: &str,
        plan: &str,
        input: &CommitExecutionPlan,
    ) -> Result<RunRecord> {
        let saved = self.store.execution_plan(order, plan).await?;
        if saved.sha256 != input.sha256 {
            return Err(Error::ExecutionPlanChanged);
        }
        if let Some(receipt) = self.store.execution_plan_run(&saved).await? {
            return Ok(receipt);
        }
        let at = now();
        if at < saved.plan.created_at || at >= saved.plan.expires_at {
            return Err(Error::ExecutionPlanExpired);
        }
        let accepted = async {
            let (flight, spec) = self.resolve_execution_intent(&saved.request).await?;
            if canonical_hash(&flight) != canonical_hash(&saved.plan.preflight)
                || canonical_hash(&spec) != saved.plan.spec_sha256
                || canonical_hash(&self.execution_boundary(&spec).await?)
                    != canonical_hash(&saved.plan.boundary)
            {
                return Err(Error::ExecutionPlanChanged);
            }
            // Keep provider edits excluded until the run and its receipt commit.
            let configured = self.providers.read().await;
            let profiles = match &flight {
                RunPreflight::Single(flight) => vec![&flight.provider],
                RunPreflight::Workflow(flight) => {
                    flight.roles.iter().map(|role| &role.provider).collect()
                }
            };
            for profile in profiles {
                if !configured
                    .iter()
                    .any(|p| canonical_hash(&p.profile) == canonical_hash(profile))
                {
                    return Err(Error::ExecutionPlanChanged);
                }
            }
            let result = self.store.create_execution_plan_run(&saved).await;
            drop(configured);
            result
        }
        .await;
        // A concurrent commit's initial receipt wins over a failed later preflight.
        if accepted.is_err()
            && let Some(receipt) = self.store.execution_plan_run(&saved).await?
        {
            return Ok(receipt);
        }
        accepted
    }

    pub async fn retire_execution_plan(
        &self,
        order: &str,
        plan: &str,
        input: &CommitExecutionPlan,
    ) -> Result<PreparedExecutionPlan> {
        let saved = self.store.execution_plan(order, plan).await?;
        if saved.sha256 != input.sha256 {
            return Err(Error::ExecutionPlanChanged);
        }
        self.store.retire_execution_plan(&saved).await?;
        self.execution_plan_view(&saved).await
    }
}
