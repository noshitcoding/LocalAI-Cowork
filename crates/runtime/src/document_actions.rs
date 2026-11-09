//! Human-operated document actions reuse model tool validation, policy and file publication.
use super::*;
use cowork_domain::documents::parse_document_call;
use cowork_storage::document_actions::StoredDocumentPreparation;

impl Runtime {
    pub async fn prepare_document_action(
        &self,
        order_id: &str,
        request: &DocumentActionRequest,
    ) -> Result<DocumentPreparation> {
        if request.version != 2 {
            return Err(Error::Validation(
                "Unsupported document action version".into(),
            ));
        }
        valid_text(&request.idempotency_key, 128)?;
        let parsed = parse_document_call(&request.tool)?;
        if request.access_mode == AccessMode::ReadOnly && parsed.output.is_some() {
            return Err(Error::Forbidden);
        }
        if let Some(prepared) = self
            .store
            .existing_document_preparation(order_id, request)
            .await?
        {
            return self.document_preparation(order_id, &prepared.view.id).await;
        }
        let order = self.store.order(order_id).await?;
        if order.revision != request.base_revision {
            return Err(Error::Conflict);
        }
        let workspace = self.workspace(order_id).await?;
        let output_folder = parsed.output.as_ref().map(|output| {
            output
                .folder_id
                .clone()
                .unwrap_or_else(|| workspace.folder.id.clone())
        });
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let mut folders = vec![workspace.folder];
        let configured = files.grants(&order.project_id)?;
        for folder in parsed
            .inputs
            .iter()
            .map(|i| &i.folder_id)
            .chain(parsed.output.iter().filter_map(|o| o.folder_id.as_ref()))
        {
            if !folders.iter().any(|f| &f.id == folder) {
                folders.push(
                    configured
                        .iter()
                        .find(|f| &f.id == folder)
                        .cloned()
                        .ok_or(Error::Forbidden)?,
                );
            }
        }
        for grant in &mut folders {
            grant.writable = grant.writable
                && request.access_mode != AccessMode::ReadOnly
                && output_folder.as_ref() == Some(&grant.id);
        }
        let mut tool = request.tool.clone();
        tool.id = "document-action".into();
        let mut spec = RunSpec {
            input_workspace: None,
            agent_config: None,
            model_binding: None,
            output: None,
            workflow: None,
            mcp_tools: vec![],
            environment: None,
            order_id: order.id.clone(),
            project_id: order.project_id,
            prompt: format!("{} · {}", tool.name, parsed.operation),
            provider: None,
            file_action: Some(tool.clone()),
            access_mode: request.access_mode.clone(),
            executor: "local-v2".into(),
            policy_revision: 1,
            allowed_tools: vec![tool.name.clone()],
            budgets: Budgets::default(),
            folders,
        };
        // Snapshot every source, including calls that did not provide an expected hash.
        let inputs = parsed.inputs.clone();
        let reading = self.document_record_for_precheck(spec.clone());
        let hashes = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            inputs
                .iter()
                .map(|input| {
                    let bytes = files.read_binary(
                        &reading,
                        &FileLocation {
                            folder_id: input.folder_id.clone(),
                            path: input.path.clone(),
                        },
                        cowork_toolbox::MAX_DOCUMENT_INPUT_BYTES,
                    )?;
                    let sha = hash(&bytes);
                    if input.expected_sha256.as_ref().is_some_and(|h| h != &sha) {
                        return Err(Error::FileConflict);
                    }
                    Ok(sha)
                })
                .collect()
        })
        .await
        .map_err(storage_error)??;
        for (index, sha) in hashes.into_iter().enumerate() {
            let target = match tool.name.as_str() {
                "document_read" => &mut tool.arguments,
                "document_write" => &mut tool.arguments["inputs"][index],
                "image_edit" if index == 0 => &mut tool.arguments["input"],
                "image_edit" => &mut tool.arguments["overlays"][index - 1],
                _ => return Err(Error::Forbidden),
            };
            target["expected_sha256"] = json!(sha);
        }
        let engine_revision = self
            .toolbox
            .as_ref()
            .ok_or(Error::Forbidden)?
            .document_engine_revision();
        tool.id = format!("document-action:{engine_revision}");
        spec.file_action = Some(tool.clone());
        let record = self.document_record_for_precheck(spec.clone());
        self.document_precheck(&record, &tool).await?;
        let revision = self.store.order(order_id).await?.revision;
        let identifier = id();
        let fingerprint = canonical_hash(&json!([
            "document-preparation/2",
            identifier,
            revision,
            engine_revision,
            spec
        ]));
        let view = DocumentPreparation {
            id: identifier,
            order_id: order_id.into(),
            base_revision: revision,
            idempotency_key: request.idempotency_key.clone(),
            access_mode: request.access_mode.clone(),
            tool,
            fingerprint,
            engine_revision,
            requires_approval: request.access_mode == AccessMode::ConfirmAll
                || (request.access_mode == AccessMode::ConfirmMutations && parsed.output.is_some()),
            created_at: now(),
            run_id: None,
        };
        Ok(self
            .store
            .retain_document_preparation(
                order_id,
                request,
                &StoredDocumentPreparation { view, spec },
            )
            .await?
            .view)
    }

    pub async fn document_preparation(
        &self,
        order: &str,
        preparation: &str,
    ) -> Result<DocumentPreparation> {
        let prepared = self.store.document_preparation(order, preparation).await?;
        let run = self.store.existing_document_run(&prepared).await?;
        Ok(DocumentPreparation {
            run_id: run.map(|r| r.id),
            ..prepared.view
        })
    }

    pub async fn document_preparation_by_key(
        &self,
        order: &str,
        key: &str,
    ) -> Result<DocumentPreparation> {
        let prepared = self.store.document_preparation_by_key(order, key).await?;
        self.document_preparation(order, &prepared.view.id).await
    }

    pub async fn commit_document_action(
        &self,
        order: &str,
        preparation: &str,
    ) -> Result<RunRecord> {
        let prepared = self.store.document_preparation(order, preparation).await?;
        if let Some(run) = self.store.existing_document_run(&prepared).await? {
            return Ok(run);
        }
        if self
            .toolbox
            .as_ref()
            .ok_or(Error::Forbidden)?
            .document_engine_revision()
            != prepared.view.engine_revision
        {
            return Err(Error::Conflict);
        }
        let record = self.document_record_for_precheck(prepared.spec.clone());
        self.restore_run_workspace(&record).await?;
        self.document_precheck(&record, &prepared.view.tool).await?;
        match self.store.create_document_run(&prepared).await {
            Err(Error::Conflict) => self
                .store
                .existing_document_run(&prepared)
                .await?
                .ok_or(Error::Conflict),
            result => result,
        }
    }

    fn document_record_for_precheck(&self, spec: RunSpec) -> RunRecord {
        // Only the spec is consulted by document_precheck and workspace validation.
        RunRecord {
            id: String::new(),
            spec,
            state: RunState::Queued,
            revision: 1,
            last_sequence: 0,
            steps: 0,
            created_at: now(),
            updated_at: now(),
            started_at: None,
            error_code: None,
            usage: Usage::default(),
            model_attempts: vec![],
        }
    }
}
