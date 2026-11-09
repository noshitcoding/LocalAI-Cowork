//! Model-free restoration uses the common immutable plan/copy/receipt path.
use super::*;
use cowork_broker::RetainedContent;
use cowork_workflow_contracts::{
    PrepareRestoreExecutionPlanV2, RestoreContentBindingV2, RestoreExecutionPreflightV2,
};

impl Runtime {
    pub async fn prepare_restore_execution_plan_v2(
        &self,
        request: &PrepareRestoreExecutionPlanV2,
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
            .resolve_restore_execution_v2(request, &id(), &id())
            .await?;
        self.retain_resolved_execution_v2(retained, resolved).await
    }

    pub async fn retire_restore_execution_request_v2(
        &self,
        request: &PrepareRestoreExecutionPlanV2,
    ) -> Result<cowork_workflow_contracts::RetiredExecutionRequest> {
        self.store
            .retire_execution_plan_v2_request(&request.clone().into())
            .await
    }

    pub(super) async fn resolve_restore_execution_v2(
        &self,
        request: &PrepareRestoreExecutionPlanV2,
        namespace: &str,
        nonce: &str,
    ) -> Result<Resolved> {
        if !matches!(request.target, OwnerExecutionTargetV2::BrokerController {}) {
            return Err(Error::Forbidden);
        }
        let mut source = self.restore_source_spec(&request.request).await?;
        let content = self
            .retained_restore_content(&source, &request.request.restore, true)
            .await?;
        let content = match content {
            RetainedContent::File { sha256 } => RestoreContentBindingV2 {
                kind: FileNodeKind::File,
                sha256,
            },
            RetainedContent::Directory { sha256, .. } => RestoreContentBindingV2 {
                kind: FileNodeKind::Directory,
                sha256,
            },
        };
        let mut restore = request.request.restore.clone();
        restore.expected_source_sha256 = Some(content.sha256.clone());
        source
            .file_action
            .as_mut()
            .ok_or(Error::Forbidden)?
            .arguments = json!(restore);
        if !broker_scope(&source) {
            return Err(Error::Forbidden);
        }
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
        let target = workspace
            .copies
            .iter()
            .find(|copy| copy.grant.id == restore.folder_id && copy.grant.writable)
            .ok_or(Error::Forbidden)?;
        cowork_domain::workspace_transfer::validate_path(&restore.path)?;
        let path = format!("{}/{}", target.alias, restore.path);
        let manifest = &stored.capture.snapshot.manifest;
        if manifest.files.iter().any(|entry| entry.path == path)
            || manifest.directories.iter().any(|entry| entry == &path)
        {
            return Err(Error::Conflict);
        }
        if let Some((parent, _)) = restore.path.rsplit_once('/') {
            let parent = format!("{}/{parent}", target.alias);
            if !manifest.directories.contains(&parent) {
                return Err(Error::NotFound);
            }
        }
        let mut spec = source.clone();
        spec.input_workspace = Some(workspace.binding.clone());
        spec.folders = workspace.folders();
        let flight = ExecutionPreflightV2::Restore {
            review: RestoreExecutionPreflightV2 {
                ready: true,
                executor: spec.executor.clone(),
                restore,
                content,
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
