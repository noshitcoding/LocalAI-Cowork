//! Local input preparation only. Public callers cannot export input bytes or
//! bypass the forthcoming confirmed-plan/recipient transfer admission.
use super::*;

impl Runtime {
    pub async fn prepare_execution_input(
        &self,
        order: &str,
        request: &PrepareExecutionInput,
    ) -> Result<PreparedExecutionInput> {
        cowork_domain::execution_inputs::validate_request(request)?;
        let owner = self.store.order(order).await?;
        if let Some(saved) = self.store.existing_execution_input(order, request).await? {
            if saved.capture.snapshot.project_id != owner.project_id {
                return Err(Error::Forbidden);
            }
            return Ok(saved.view());
        }
        if owner.revision != request.base_revision {
            return Err(Error::Conflict);
        }
        if request.folder_ids.is_empty() {
            let capture = cowork_domain::execution_inputs::CapturedExecutionInput::empty(
                &owner,
                request,
                &id(),
                now(),
            )?;
            return Ok(self
                .store
                .retain_execution_input(request, &capture)
                .await?
                .view());
        }
        // Rehydrate only the current native workspace capability when selected.
        // Ordinary registered folders are resolved and checked inside the broker.
        if request
            .folder_ids
            .iter()
            .any(|id| id.starts_with("workspace:"))
        {
            self.workspace(order).await?;
        }
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let request = request.clone();
        let capture_request = request.clone();
        let permit = self.begin_upload_transfer()?;
        let prepared = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            files.prepare_execution_input(&owner, &capture_request, &id(), now())
        })
        .await
        .map_err(storage_error)??;
        // prepared owns native read/revocation locks across the SQL commit.
        let saved = self
            .store
            .retain_execution_input(&request, &prepared.capture)
            .await?;
        drop(prepared);
        Ok(saved.view())
    }

    pub async fn inspect_execution_input(
        &self,
        order: &str,
        snapshot: &str,
    ) -> Result<PreparedExecutionInput> {
        let owner = self.store.order(order).await?;
        let saved = self.store.execution_input(order, snapshot).await?;
        if saved.capture.snapshot.project_id != owner.project_id {
            return Err(Error::Forbidden);
        }
        Ok(saved.view())
    }

    pub async fn retire_execution_input(
        &self,
        order: &str,
        snapshot: &str,
        request: &RetireExecutionInput,
    ) -> Result<PreparedExecutionInput> {
        self.inspect_execution_input(order, snapshot).await?;
        Ok(self
            .store
            .retire_execution_input(order, snapshot, &request.sha256)
            .await?
            .view())
    }
}
