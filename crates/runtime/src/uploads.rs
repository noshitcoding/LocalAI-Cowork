use super::*;

impl Runtime {
    /// Acquire before collecting an HTTP body, not after buffering it.
    pub fn begin_upload_transfer(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        self.upload_transfers
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::TransferBusy)
    }
    pub async fn reserve_upload(&self, order: &str, input: &ReserveUpload) -> Result<FileUpload> {
        self.files.as_ref().ok_or(Error::Forbidden)?;
        cowork_broker::FileBroker::validate_upload_metadata(&input.file)?;
        self.store.reserve_upload(order, input).await
    }
    pub async fn receive_upload(
        &self,
        order: &str,
        upload: &str,
        bytes: Vec<u8>,
    ) -> Result<FileUpload> {
        let permit = self.begin_upload_transfer()?;
        self.receive_upload_with_permit(order, upload, bytes, permit)
            .await
    }
    pub async fn receive_upload_with_permit(
        &self,
        order: &str,
        upload: &str,
        bytes: Vec<u8>,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<FileUpload> {
        let record = self.store.upload(order, upload).await?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            files.retain_upload(&record, &bytes)
        })
        .await
        .map_err(storage_error)??;
        self.store.confirm_upload(order, upload).await
    }
    pub(super) async fn import_source(
        &self,
        spec: &RunSpec,
        input: &ImportFile,
    ) -> Result<FileUpload> {
        let record = self.store.upload(&spec.order_id, &input.upload_id).await?;
        if record.state != UploadState::Ready
            || record.project_id != spec.project_id
            || canonical_hash(&record.file) != canonical_hash(&input.file)
            || !spec.folders.iter().any(|f| {
                f.id == input.folder_id && f.writable && f.order_id.as_ref() == Some(&spec.order_id)
            })
        {
            return Err(Error::Forbidden);
        }
        Ok(record)
    }
    pub async fn create_import_run(&self, input: &CreateImportRun) -> Result<RunRecord> {
        if let Some(existing) = self.store.existing_import_request(input).await? {
            return Ok(existing);
        }
        let created = self.prepare_import_run(input).await;
        if matches!(created, Err(Error::Conflict))
            && let Some(existing) = self.store.existing_import_request(input).await?
        {
            return Ok(existing);
        }
        created
    }
    async fn prepare_import_run(&self, input: &CreateImportRun) -> Result<RunRecord> {
        let spec = self.import_source_spec(input).await?;
        self.store.create_import_run(input, spec).await
    }
    pub(super) async fn import_source_spec(&self, input: &CreateImportRun) -> Result<RunSpec> {
        valid_text(&input.prompt, 64_000)?;
        valid_text(&input.idempotency_key, 128)?;
        if input.access_mode == AccessMode::ReadOnly {
            return Err(Error::Forbidden);
        }
        let order = self.store.order(&input.order_id).await?;
        if order.revision != input.base_revision {
            return Err(Error::Conflict);
        }
        cowork_broker::relative_path(&input.import.path, false)?;
        let workspace = self.workspace(&order.id).await?;
        if workspace.folder.id != input.import.folder_id {
            return Err(Error::Forbidden);
        }
        let spec = RunSpec {
            input_workspace: None,
            agent_config: None,
            model_binding: None,
            output: None,
            workflow: None,
            mcp_tools: vec![],
            environment: None,
            order_id: order.id,
            project_id: order.project_id,
            prompt: input.prompt.clone(),
            provider: None,
            file_action: Some(ToolCall {
                id: "import".into(),
                name: "import_file".into(),
                arguments: json!(input.import),
            }),
            access_mode: input.access_mode.clone(),
            executor: "local-v2".into(),
            policy_revision: 1,
            allowed_tools: vec!["import_file".into()],
            budgets: Budgets::default(),
            folders: vec![workspace.folder],
        };
        let upload = self.import_source(&spec, &input.import).await?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        tokio::task::spawn_blocking(move || files.verify_upload(&upload))
            .await
            .map_err(storage_error)??;
        Ok(spec)
    }
}
