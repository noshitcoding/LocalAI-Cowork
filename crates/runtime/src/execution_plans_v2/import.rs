//! A confirmed import writes a staged upload into one fresh independent copy.
use super::*;
use cowork_workflow_contracts::{ImportExecutionPreflightV2, PrepareImportExecutionPlanV2};

impl Runtime {
    pub async fn prepare_import_execution_plan_v2(
        &self,
        request: &PrepareImportExecutionPlanV2,
    ) -> Result<PreparedExecutionPlanV2> {
        let retained: StoredExecutionRequestV2 = request.clone().into();
        if let Some(saved) = self
            .store
            .existing_execution_plan_v2_request(&retained)
            .await?
        {
            return self.store.execution_plan_v2_view(&saved).await;
        }
        let resolved = self
            .resolve_import_execution_v2(request, &id(), &id())
            .await?;
        self.retain_resolved_execution_v2(retained, resolved).await
    }
    pub async fn retire_import_execution_request_v2(
        &self,
        request: &PrepareImportExecutionPlanV2,
    ) -> Result<cowork_workflow_contracts::RetiredExecutionRequest> {
        self.store
            .retire_execution_plan_v2_request(&request.clone().into())
            .await
    }
    pub(super) async fn resolve_import_execution_v2(
        &self,
        request: &PrepareImportExecutionPlanV2,
        namespace: &str,
        nonce: &str,
    ) -> Result<Resolved> {
        if !matches!(request.target, OwnerExecutionTargetV2::BrokerController {}) {
            return Err(Error::Forbidden);
        }
        let source = self.import_source_spec(&request.request).await?;
        if !broker_scope(&source) {
            return Err(Error::Forbidden);
        }
        let upload = self.import_source(&source, &request.request.import).await?;
        let stored = self
            .store
            .execution_input(&source.order_id, &request.input.snapshot_id)
            .await?;
        if stored.retired
            || stored.capture.sha256 != request.input.sha256
            || stored.capture.snapshot.manifest.sha256 != request.input.manifest_sha256
        {
            return Err(Error::ExecutionPlanChanged);
        }
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let origin = tokio::task::spawn_blocking(move || files.local_controller_origin())
            .await
            .map_err(storage_error)??;
        let controller = self.store.broker_controller(&origin).await?;
        let workspace = RunWorkspaceIntent::for_spec(&stored.capture, &source, namespace, nonce)?;
        let action = &request.request.import;
        let target = workspace
            .copies
            .iter()
            .find(|copy| copy.grant.id == action.folder_id && copy.grant.writable)
            .ok_or(Error::Forbidden)?;
        cowork_domain::workspace_transfer::validate_path(&action.path)?;
        let path = format!("{}/{}", target.alias, action.path);
        let manifest = &stored.capture.snapshot.manifest;
        if manifest.directories.iter().any(|entry| entry == &path) {
            return Err(Error::Conflict);
        }
        let existing = manifest.files.iter().find(|entry| entry.path == path);
        match (&action.expected_sha256, existing) {
            (None, None) => {}
            (Some(expected), Some(entry)) if expected == &entry.sha256 => {}
            _ => return Err(Error::Conflict),
        }
        if let Some((parent, _)) = action.path.rsplit_once('/')
            && !manifest
                .directories
                .contains(&format!("{}/{parent}", target.alias))
        {
            return Err(Error::NotFound);
        }
        let mut spec = source.clone();
        spec.input_workspace = Some(workspace.binding.clone());
        spec.folders = workspace.folders();
        let flight = ExecutionPreflightV2::Import {
            review: ImportExecutionPreflightV2 {
                ready: true,
                executor: spec.executor.clone(),
                import: action.clone(),
                upload,
                folders: spec.folders.clone(),
                allowed_tools: spec.allowed_tools.clone(),
                access_mode: spec.access_mode.clone(),
                budgets: spec.budgets.clone(),
            },
        };
        Ok(Resolved {
            source,
            spec,
            flight,
            capture: stored.capture,
            workspace,
            origin,
            destination: ExecutionDestinationV2::BrokerController { controller },
        })
    }
}
