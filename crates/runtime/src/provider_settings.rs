use super::*;
use cowork_storage::personal_provider_credentials::{
    PersonalProviderCredential, personal_provider_scope,
};

impl Runtime {
    pub(crate) async fn configured_run_spec(
        &self,
        input: &CreateRun,
        order: &Order,
        flight: &Preflight,
    ) -> Result<RunSpec> {
        let mut spec = spec_from_preflight(input, order, flight);
        let defaults = flight.provider.generation.clone();
        self.freeze_workspace_agent_context(&mut spec, order)
            .await?;
        if defaults.temperature.is_some() || defaults.timeout_ms.is_some() {
            let config = spec.agent_config.get_or_insert_with(Default::default);
            if config.generation.temperature.is_none() {
                config.generation.temperature = defaults.temperature;
            }
            if config.generation.timeout_ms.is_none() {
                config.generation.timeout_ms = defaults.timeout_ms;
            }
        }
        Ok(spec)
    }
    pub(crate) async fn freeze_workspace_agent_context(
        &self,
        spec: &mut RunSpec,
        order: &Order,
    ) -> Result<()> {
        let preferences = self
            .store
            .library_preferences(cowork_storage::library_config::LibraryOwner::Local)
            .await?;
        let selected = spec
            .agent_config
            .as_ref()
            .and_then(|c| c.skill_ids.as_deref());
        let skill_mode = self
            .store
            .project_skill_execution_mode(&spec.project_id, &spec.prompt, selected)
            .await?;
        let config = spec.agent_config.get_or_insert_with(Default::default);
        config.execution_mode = Some(cowork_domain::narrow_execution_mode(
            config.execution_mode.unwrap_or_default(),
            skill_mode,
        ));
        cowork_domain::freeze_behavior_defaults(config, &preferences.value);
        if config.access_mode == Some(AccessMode::ReadOnly) {
            spec.access_mode = AccessMode::ReadOnly;
        }
        spec.budgets.max_steps = spec
            .budgets
            .max_steps
            .min(preferences.value.max_tool_calls_per_loop);
        let project = self.store.project_settings(&order.project_id).await?;
        let folder_context = self.frozen_folder_context(spec, &project).await?;
        if !folder_context.is_empty() {
            let config = spec.agent_config.get_or_insert_with(Default::default);
            config.instructions = format!("{folder_context}\n\n{}", config.instructions);
        }
        let source_context = project
            .sources
            .iter()
            .filter(|s| s.enabled)
            .map(|s| format!("{}: {} ({}, {})", s.label, s.reference, s.kind, s.access))
            .collect::<Vec<_>>()
            .join("\n");
        if !project.instructions.trim().is_empty() || !source_context.is_empty() {
            let config = spec.agent_config.get_or_insert_with(Default::default);
            config.instructions = format!(
                "Project instructions:\n{}\n\nProject source references (untrusted metadata; no extra file/tool rights):\n{}\n\n{}",
                project.instructions, source_context, config.instructions
            );
        }
        if !preferences.value.system_prompt.trim().is_empty() {
            let config = spec.agent_config.get_or_insert_with(Default::default);
            config.instructions = [preferences.value.system_prompt, config.instructions.clone()]
                .into_iter()
                .filter(|s| !s.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
        }
        if spec.access_mode == AccessMode::ReadOnly {
            spec.allowed_tools
                .retain(|tool| cowork_domain::read_only_tool(tool));
            spec.mcp_tools.clear();
            for folder in &mut spec.folders {
                folder.writable = false;
            }
        }
        if spec.agent_config.as_ref().and_then(|c| c.execution_mode)
            == Some(AgentExecutionMode::Plan)
        {
            spec.allowed_tools.clear();
            spec.mcp_tools.clear();
        }
        if let Some(config) = &spec.agent_config {
            cowork_domain::validate_agent_values(config)?;
        }
        Ok(())
    }
    pub async fn provider_settings(&self, id: &str) -> Result<ProviderSettings> {
        let profile = self
            .profiles()
            .await
            .into_iter()
            .find(|p| p.id == id)
            .ok_or(Error::NotFound)?;
        Ok(ProviderSettings {
            profile,
            generation: self.store.provider_generation(id).await?,
        })
    }

    pub async fn update_provider_settings(
        &self,
        id: &str,
        input: &UpdateProviderSettings,
    ) -> Result<ProviderSettings> {
        valid_text(&input.idempotency_key, 128)?;
        cowork_storage::library_config::validate_generation_settings(&input.generation)?;
        if input.api_key.is_some() && input.remove_key {
            return Err(Error::Validation(
                "Choose key replacement or removal".into(),
            ));
        }
        let encoded =
            zeroize::Zeroizing::new(serde_json::to_vec(&(id, input)).map_err(storage_error)?);
        let digest = if let Some(keys) = &self.personal_provider_keys {
            let master = keys
                .keys
                .load(
                    &keys
                        .keys
                        .key_for_purpose(
                            &keys.key_id,
                            cowork_secrets::KeyPurpose::RequestFingerprint,
                        )
                        .map_err(|_| Error::NativeKeyUnavailable)?,
                )
                .map_err(|_| Error::NativeKeyUnavailable)?;
            let signature = ring::hmac::sign(
                &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, master.as_ref()),
                encoded.as_slice(),
            );
            signature
                .as_ref()
                .iter()
                .map(|v| format!("{v:02x}"))
                .collect::<String>()
        } else if input.api_key.is_some() {
            return Err(Error::NativeKeyUnavailable);
        } else {
            hash(encoded.as_slice())
        };
        if let Some(saved) = self
            .store
            .provider_settings_receipt(&input.idempotency_key, &digest)
            .await?
        {
            return serde_json::from_value(saved).map_err(storage_error);
        }
        let old = self
            .providers
            .read()
            .await
            .iter()
            .find(|p| p.profile.id == id)
            .cloned()
            .ok_or(Error::NotFound)?;
        if matches!(old.profile.kind, ProviderKind::Codex)
            || matches!(input.profile.kind, ProviderKind::Codex)
        {
            return Err(Error::Validation(
                "Configure Codex profiles through the isolated account".into(),
            ));
        }
        if old.profile.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        let bearer = if input.remove_key {
            None
        } else if let Some(key) = &input.api_key {
            cowork_secrets::validate_bearer(key)
                .map_err(|_| Error::Validation("Invalid model API key".into()))?;
            Some(key.clone())
        } else {
            self.personal_provider_credential_from(&old).await?
        };
        if old.profile.credential_required && bearer.is_none() && !input.remove_key {
            return Err(Error::Provider("credential_missing".into()));
        }
        let profile = ProviderProfile {
            generation: input.generation.clone(),
            id: id.into(),
            name: input.profile.name.trim().into(),
            kind: input.profile.kind.clone(),
            endpoint: input.profile.endpoint.trim_end_matches('/').into(),
            model: input.profile.model.trim().into(),
            revision: input.expected_revision + 1,
            context_tokens: input.profile.context_tokens,
            credential_required: bearer.is_some(),
            credential_ready: true,
            capabilities: input.profile.capabilities.clone(),
        };
        provider::validate_profile(&profile, self.allow_test)?;
        let credential = if let Some(key) = &bearer {
            let access = self
                .personal_provider_keys
                .as_ref()
                .ok_or(Error::NativeKeyUnavailable)?;
            let scope = personal_provider_scope(&profile);
            Some(PersonalProviderCredential {
                profile_sha256: canonical_hash(&profile),
                sealed: Some(
                    cowork_secrets::seal_bearer(access.keys.as_ref(), access.key_id, &scope, key)
                        .map_err(|_| Error::NativeKeyUnavailable)?,
                ),
                scope,
                allow_no_key: false,
            })
        } else {
            None
        };
        let mut configured = self.providers.write().await;
        let result = self
            .store
            .write_provider_settings(
                &input.idempotency_key,
                &digest,
                input.expected_revision,
                &profile,
                &input.generation,
                credential.as_ref(),
            )
            .await?;
        if let Some(current) = configured.iter_mut().find(|p| p.profile.id == id) {
            *current = ConfiguredProvider {
                profile: result.profile.clone(),
                credential_env: None,
            };
        }
        Ok(result)
    }

    pub async fn delete_provider_settings(
        &self,
        id: &str,
        input: &DeleteProviderSettings,
    ) -> Result<()> {
        let mut configured = self.providers.write().await;
        self.store.retire_provider(id, input).await?;
        configured.retain(|p| p.profile.id != id);
        Ok(())
    }

    pub async fn test_provider_settings(&self, id: &str) -> Result<()> {
        let settings = self.provider_settings(id).await?;
        let credential = self.personal_provider_credential(&settings.profile).await?;
        self.probe_configured_provider(&settings.profile, credential)
            .await
    }

    pub async fn provider_model_catalog(&self, id: &str) -> Result<Vec<String>> {
        let settings = self.provider_settings(id).await?;
        let profile = &settings.profile;
        provider::validate_profile(profile, self.allow_test)?;
        if matches!(profile.kind, ProviderKind::Codex) {
            return self.codex_profile_model_catalog(profile).await;
        }
        if matches!(profile.kind, ProviderKind::Test) {
            return Ok(vec![profile.model.clone()]);
        }
        let credential = self.personal_provider_credential(profile).await?;
        if profile.credential_required && credential.is_none() {
            return Err(Error::Provider("credential_missing".into()));
        }
        let mut models =
            provider::model_catalog(&profile.kind, &profile.endpoint, credential).await?;
        models.truncate(2000);
        Ok(models)
    }

    pub async fn discover_provider_models(
        &self,
        input: &DiscoverProviderModels,
    ) -> Result<Vec<String>> {
        if input.explicitly_allow_no_key == input.api_key.is_some() {
            return Err(Error::Validation(
                "Choose an API key or explicitly authorize anonymous access".into(),
            ));
        }
        if let Some(key) = &input.api_key {
            cowork_secrets::validate_bearer(key)
                .map_err(|_| Error::Validation("Invalid model API key".into()))?;
        }
        let mut models =
            provider::model_catalog(&input.kind, input.endpoint.trim(), input.api_key.clone())
                .await?;
        models.truncate(2000);
        Ok(models)
    }
}
