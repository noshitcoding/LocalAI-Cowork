//! Product adapter for trusted exact models and explicitly archived compaction.
use super::*;
use cowork_storage::StoredModelContext;
impl Runtime {
    pub(crate) async fn prepare_model(
        &self,
        spec: &RunSpec,
        messages: &[Value],
    ) -> Result<provider::PreparedModelRequest> {
        let registered = self.store.registered_model(spec).await?;
        if let Some(model) = &registered {
            self.assert_model_deployment(spec, model).await?;
        }
        // Boxed: prepare_model is awaited inline by execute; keep its state machine small.
        let images = Box::pin(self.request_images(spec, messages)).await?;
        let mut prepared = provider::prepare_registered_images(
            spec,
            messages,
            self.allow_test,
            registered.as_ref(),
            &images,
        )?;
        if let Some(profile) = spec
            .provider
            .as_ref()
            .filter(|p| matches!(p.kind, ProviderKind::Codex))
        {
            prepared.bind_codex_transport(
                provider::codex_gateway::transport(
                    &self.store,
                    self.codex_broker()
                        .ok_or_else(|| Error::Provider("codex_runtime_not_configured".into()))?,
                    "installation-owner".into(),
                    profile,
                )
                .await?,
            )?;
        }
        Ok(prepared)
    }
    /// Loads the SHA-verified preview images referenced by recent tool results of a vision model.
    async fn request_images(
        &self,
        spec: &RunSpec,
        messages: &[Value],
    ) -> Result<provider::RequestImages> {
        let vision = spec
            .provider
            .as_ref()
            .is_some_and(|p| p.capabilities.image_input)
            && spec.model_binding.is_none();
        let (Some(files), true) = (self.files.clone(), vision) else {
            return Ok(Default::default());
        };
        let wanted: Vec<(String, String)> = messages
            .iter()
            .rev()
            .filter(|m| m["role"] == "tool")
            .flat_map(|m| m["cowork_images"].as_array().cloned().unwrap_or_default())
            .filter_map(|r| {
                Some((
                    r["sha256"].as_str()?.to_owned(),
                    r["media_type"].as_str().unwrap_or("image/png").to_owned(),
                ))
            })
            .take(16)
            .collect();
        if wanted.is_empty() {
            return Ok(Default::default());
        }
        tokio::task::spawn_blocking(move || {
            // A missing preview becomes a placeholder; it never blocks the conversation.
            wanted
                .into_iter()
                .filter_map(|(sha, media_type)| {
                    files
                        .read_preview(&sha)
                        .ok()
                        .map(|bytes| (sha, (media_type, bytes)))
                })
                .collect()
        })
        .await
        .map_err(storage_error)
    }
    async fn assert_model_deployment(
        &self,
        spec: &RunSpec,
        model: &cowork_model_accounting::registry::RegisteredModel,
    ) -> Result<()> {
        provider::assert_qualified_deployment(
            spec,
            model,
            self.workflow_credential(spec).await?.as_deref(),
        )
        .await
    }
    async fn context_source(&self, input: &CreateRun) -> Result<StoredModelContext> {
        let flight = self.preflight_context_scope(input, false).await?;
        let order = self.store.order(&input.order_id).await?;
        self.store
            .model_context_source(&self.configured_run_spec(input, &order, &flight).await?)
            .await
    }
    async fn measure_context(&self, source: &StoredModelContext) -> Result<ModelContextView> {
        let messages: Vec<_> = source
            .groups
            .iter()
            .flat_map(|g| g.messages.clone())
            .collect();
        let tools = provider::tool_catalog(&source.spec);
        let profile = source.spec.provider.as_ref().ok_or(Error::Forbidden)?;
        let output_allowance = u64::from(source.spec.budgets.max_output_tokens);
        let (count_mode, input_tokens, conservative_bytes, fits) = if let Some(model) =
            self.store.registered_model(&source.spec).await?
        {
            let logical = provider::request_fingerprint(&source.spec, &messages)?;
            let prompt = model
                .prompt(
                    &messages,
                    &tools,
                    &serde_json::to_value(&source.spec.output).map_err(storage_error)?,
                    &logical,
                )
                .map_err(storage_error)?;
            let count = model
                .tokenizer
                .measure(&prompt, output_allowance)
                .map_err(storage_error)?;
            let fits = count
                .input_tokens
                .checked_add(output_allowance)
                .is_some_and(|n| n <= u64::from(profile.context_tokens));
            (
                "exact_artifact_renderer",
                Some(count.input_tokens),
                None,
                fits,
            )
        } else {
            let mut request = json!({"format":1,"provider":profile,"messages":messages,"tools":tools,"max_output_tokens":source.spec.budgets.max_output_tokens});
            if let Some(output) = &source.spec.output {
                request["output"] = serde_json::to_value(output).map_err(storage_error)?;
            }
            let bytes = request
                .to_string()
                .len()
                .saturating_add(messages.len().saturating_mul(128)) as u64;
            (
                "conservative_bytes",
                None,
                Some(bytes),
                bytes
                    .checked_add(output_allowance)
                    .is_some_and(|n| n <= u64::from(profile.context_tokens)),
            )
        };
        Ok(ModelContextView {
            order_id: source.spec.order_id.clone(),
            order_revision: source.order_revision,
            snapshot_sha256: canonical_hash(source),
            groups: source
                .groups
                .iter()
                .map(|g| ContextGroupView {
                    id: g.id.clone(),
                    sha256: g.sha256.clone(),
                    required: g.required,
                    message_count: g.messages.len() as u32,
                    characters: g.messages.iter().map(|m| m.to_string().len() as u64).sum(),
                    preview: g
                        .messages
                        .iter()
                        .filter_map(|m| m["content"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                        .chars()
                        .take(800)
                        .collect(),
                })
                .collect(),
            count_mode: count_mode.into(),
            input_tokens,
            conservative_bytes,
            output_allowance,
            context_limit: u64::from(profile.context_tokens),
            fits,
            model_binding: source.spec.model_binding.clone(),
            selection_revision: source.selection_revision,
            archive_id: source.archive_id.clone(),
        })
    }
    pub async fn preview_model_context(
        &self,
        input: &PreviewModelContext,
    ) -> Result<ModelContextView> {
        let source = self.context_source(&input.request).await?;
        self.measure_context(&source).await
    }
    pub async fn compact_model_context(
        &self,
        input: &CompactModelContext,
    ) -> Result<ContextCompactionView> {
        let request = canonical_hash(input);
        if let Some(existing) = self
            .store
            .existing_context_compaction(&input.request.order_id, &input.idempotency_key, &request)
            .await?
        {
            return Ok(existing);
        }
        let source = self.context_source(&input.request).await?;
        if canonical_hash(&source) != input.snapshot_sha256 {
            return Err(Error::Conflict);
        }
        let mut retained = source.clone();
        retained
            .groups
            .retain(|g| !input.remove_groups.contains(&g.id));
        if let Some(summary) = &input.summary {
            retained
                .groups
                .retain(|g| g.id != "required:explicit-summary");
            retained
                .groups
                .push(cowork_storage::context_summary_group(summary));
        }
        let measurement = self.measure_context(&retained).await?;
        if !measurement.fits {
            return Err(Error::ContextLimit);
        }
        self.store
            .commit_context_compaction(
                input,
                &source,
                &retained,
                &measurement.count_mode,
                measurement.input_tokens,
            )
            .await
    }
}
