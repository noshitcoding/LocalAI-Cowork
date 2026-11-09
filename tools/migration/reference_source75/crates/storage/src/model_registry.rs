//! Trusted operator import is accepted only by the native daemon boundary.
use super::*;
use cowork_model_accounting::registry::{QualificationBundle, RawWireMode, RegisteredModel};

pub(crate) fn accounting(e: cowork_model_accounting::Error) -> Error {
    match e {
        cowork_model_accounting::Error::ContextLimit => Error::ContextLimit,
        _ => Error::Validation(e.to_string()),
    }
}
pub(crate) async fn check_binding(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    spec: &RunSpec,
) -> Result<Option<RegisteredModel>> {
    let Some(binding) = &spec.model_binding else {
        return Ok(None);
    };
    let provider = spec.provider.as_ref().ok_or(Error::Forbidden)?;
    let row = sqlx::query(
        "SELECT bundle_sha256,payload,revoked FROM model_registry WHERE id=$1 AND version=$2",
    )
    .bind(&binding.id)
    .bind(i64::try_from(binding.version).map_err(storage_error)?)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .ok_or(Error::NotFound)?;
    let payload: String = row.try_get("payload").map_err(storage_error)?;
    let bundle: QualificationBundle = parse(&payload)?;
    if row.try_get::<i64, _>("revoked").map_err(storage_error)? != 0
        || row
            .try_get::<String, _>("bundle_sha256")
            .map_err(storage_error)?
            != binding.sha256
        || cowork_model_accounting::digest(&bundle).map_err(accounting)? != binding.sha256
        || bundle.provider_sha256 != canonical_hash(provider)
        || bundle.qualification.model.provider_id != provider.id
        || bundle.qualification.model.model_id != provider.model
        || bundle.qualification.model.context_tokens != u64::from(provider.context_tokens)
        || !matches!(
            (&bundle.wire_mode, &provider.kind),
            (RawWireMode::OllamaGenerate, ProviderKind::Ollama)
                | (
                    RawWireMode::OpenaiCompletions,
                    ProviderKind::OpenaiCompatible
                )
        )
    {
        return Err(Error::Forbidden);
    }
    Ok(Some(RegisteredModel::load(bundle).map_err(accounting)?))
}
pub(crate) fn view(bundle: &QualificationBundle, time: i64, revoked: bool) -> Result<ModelRegistryView> {
    let m = &bundle.qualification.model;
    Ok(ModelRegistryView {
        binding: ModelRegistryRef {
            id: bundle.id.clone(),
            version: bundle.version,
            sha256: cowork_model_accounting::digest(bundle).map_err(accounting)?,
        },
        provider_id: m.provider_id.clone(),
        provider_revision: 0,
        provider_sha256: bundle.provider_sha256.clone(),
        deployment_sha256: bundle.evidence.deployment_sha256.clone(),
        model_sha256: m.model_sha256.clone(),
        tokenizer_sha256: m.tokenizer_sha256.clone(),
        chat_template_sha256: m.chat_template_sha256.clone(),
        evidence_sha256: bundle.qualification.parity_evidence_sha256.clone(),
        context_tokens: m.context_tokens,
        wire_mode: match bundle.wire_mode {
            RawWireMode::OllamaGenerate => "ollama_generate",
            RawWireMode::OpenaiCompletions => "openai_completions",
        }
        .into(),
        reference_implementation: bundle.evidence.reference_implementation.clone(),
        registered_at: time,
        revoked,
    })
}
impl Store {
    pub(crate) async fn verify_context_admission(
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        stored: &StoredRun,
        change: &mut Change,
        intent: Option<&crate::model_money::AccountingIntent<'_>>,
    ) -> Result<()> {
        let Some(crate::model_money::AccountingIntent::Admit {
            attempt_id,
            logical,
            ..
        }) = intent
        else {
            return Ok(());
        };
        let turn = change.workflow_write.as_ref().and_then(|w| {
            w.next
                .agents
                .values()
                .flat_map(|a| &a.turns)
                .find(|t| t.id == *attempt_id)
        });
        let spec = turn.map(|t| &t.context.spec).unwrap_or(&stored.record.spec);
        let Some(model) = check_binding(tx, spec).await? else {
            return Ok(());
        };
        let messages = turn
            .map(|t| t.context.messages.as_slice())
            .unwrap_or(&stored.checkpoint.messages);
        let tools = model_tool_catalog(spec);
        if model_request_fingerprint(spec, messages, &tools)? != *logical {
            return Err(Error::Forbidden);
        }
        let prompt = model
            .prompt(
                messages,
                &tools,
                &serde_json::to_value(&spec.output).map_err(storage_error)?,
                logical,
            )
            .map_err(accounting)?;
        let count = model
            .tokenizer
            .count(&prompt, u64::from(spec.budgets.max_output_tokens))
            .map_err(accounting)?;
        let attempt = stored
            .record
            .model_attempts
            .iter()
            .find(|a| a.id == *attempt_id)
            .ok_or(Error::Forbidden)?;
        let expected = count
            .input_tokens
            .checked_add(count.output_allowance)
            .ok_or(Error::Forbidden)?;
        if attempt.reserved_tokens != expected {
            return Err(Error::Forbidden);
        }
        let binding = spec.model_binding.as_ref().ok_or(Error::Forbidden)?;
        if sqlx::query("UPDATE model_registry SET revoked=revoked WHERE id=$1 AND version=$2 AND bundle_sha256=$3 AND revoked=0").bind(&binding.id).bind(i64::try_from(binding.version).map_err(storage_error)?).bind(&binding.sha256).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Forbidden)}
        change.events.push(("qualified_model_context".into(),json!({"attempt_id":attempt_id,"binding":spec.model_binding,"count":count,"independently_verified":true})));
        Ok(())
    }
    pub async fn registered_model(&self, spec: &RunSpec) -> Result<Option<RegisteredModel>> {
        let mut tx = self.snapshot_transaction().await?;
        let model = check_binding(&mut tx, spec).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(model)
    }
    pub async fn import_model_qualification(
        &self,
        input: &ImportModelQualification,
    ) -> Result<ModelRegistryView> {
        valid_text(&input.idempotency_key, 128)?;
        let request = canonical_hash(input);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query("UPDATE model_registry_guard SET revision=revision WHERE id=1")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        // A lost acknowledgement is resolved from the original immutable
        // request receipt before current profile/revocation admission checks.
        if let Some(row) =
            sqlx::query("SELECT request_sha256,receipt FROM model_registry_operations WHERE id=$1")
                .bind(&input.idempotency_key)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
        {
            if row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != request
            {
                return Err(Error::Conflict);
            }
            let receipt = parse(&row.try_get::<String, _>("receipt").map_err(storage_error)?)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(receipt);
        }
        let bundle: QualificationBundle =
            serde_json::from_value(input.qualification_bundle.clone()).map_err(storage_error)?;
        let model = RegisteredModel::load(bundle).map_err(accounting)?;
        let bundle = &model.bundle;
        let profile_payload:Option<String>=sqlx::query_scalar("SELECT payload FROM providers WHERE id=$1").bind(&bundle.qualification.model.provider_id).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let profile:ProviderProfile=parse(&profile_payload.ok_or(Error::Forbidden)?)?;
        if canonical_hash(&profile)!=bundle.provider_sha256{return Err(Error::Forbidden)}
        let spec = RunSpec {
            model_binding: None,
            output: None,
            workflow: None,
            mcp_tools: vec![],
            environment: None,
            order_id: String::new(),
            project_id: String::new(),
            prompt: String::new(),
            provider: Some(profile.clone()),
            file_action: None,
            access_mode: AccessMode::ReadOnly,
            executor: String::new(),
            policy_revision: 1,
            allowed_tools: vec![],
            budgets: Budgets::default(),
            folders: vec![],
        };
        if bundle.qualification.model.provider_id != profile.id
            || bundle.qualification.model.model_id != profile.model
            || bundle.qualification.model.context_tokens != u64::from(profile.context_tokens)
            || !matches!(
                (&bundle.wire_mode, &profile.kind),
                (RawWireMode::OllamaGenerate, ProviderKind::Ollama)
                    | (
                        RawWireMode::OpenaiCompletions,
                        ProviderKind::OpenaiCompatible
                    )
            )
        {
            return Err(Error::Forbidden);
        }
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT bundle_sha256 FROM model_registry WHERE id=$1 AND version=$2",
        )
        .bind(&bundle.id)
        .bind(i64::try_from(bundle.version).map_err(storage_error)?)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
        let mut time = now();
        let digest = model.sha256().map_err(accounting)?;
        if let Some(existing) = existing {
            if existing != digest {
                return Err(Error::Conflict);
            }
            time = sqlx::query_scalar(
                "SELECT registered_at FROM model_registry WHERE id=$1 AND version=$2",
            )
            .bind(&bundle.id)
            .bind(i64::try_from(bundle.version).map_err(storage_error)?)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        } else {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM model_registry")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
            if count >= 1024 {
                return Err(Error::Validation("model_registry_retention_limit".into()));
            }
            let highest: Option<i64> =
                sqlx::query_scalar("SELECT MAX(version) FROM model_registry WHERE id=$1")
                    .bind(&bundle.id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            if highest.unwrap_or(0) >= i64::try_from(bundle.version).map_err(storage_error)? {
                return Err(Error::Conflict);
            }
            sqlx::query("INSERT INTO model_registry(id,version,provider_id,provider_sha256,bundle_sha256,payload,registered_at,revoked) VALUES($1,$2,$3,$4,$5,$6,$7,0)").bind(&bundle.id).bind(i64::try_from(bundle.version).map_err(storage_error)?).bind(&profile.id).bind(&bundle.provider_sha256).bind(&digest).bind(json_string(bundle)?).bind(time).execute(&mut *tx).await.map_err(storage_error)?;
        }
        let mut selected = spec;
        selected.model_binding = Some(ModelRegistryRef {
            id: bundle.id.clone(),
            version: bundle.version,
            sha256: digest,
        });
        check_binding(&mut tx, &selected).await?;
        let mut result = view(bundle, time, false)?;
        result.provider_revision = profile.revision;
        sqlx::query(
            "INSERT INTO model_registry_operations(id,request_sha256,receipt) VALUES($1,$2,$3)",
        )
        .bind(&input.idempotency_key)
        .bind(request)
        .bind(json_string(&result)?)
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn model_registry(&self) -> Result<Vec<ModelRegistryView>> {
        let rows=sqlx::query("SELECT payload,bundle_sha256,registered_at,revoked FROM model_registry ORDER BY id,version LIMIT 1024").fetch_all(&self.pool).await.map_err(storage_error)?;
        let profiles = self.providers().await?;
        rows.into_iter()
            .map(|row| {
                let bundle: QualificationBundle =
                    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                let mut result = view(
                    &bundle,
                    row.try_get("registered_at").map_err(storage_error)?,
                    row.try_get::<i64, _>("revoked").map_err(storage_error)? != 0,
                )?;
                if result.binding.sha256
                    != row
                        .try_get::<String, _>("bundle_sha256")
                        .map_err(storage_error)?
                {
                    return Err(Error::Forbidden);
                }
                if let Some(p) = profiles
                    .iter()
                    .find(|p| canonical_hash(p) == bundle.provider_sha256)
                {
                    result.provider_revision = p.revision;
                } else {
                    result.revoked = true;
                }
                Ok(result)
            })
            .collect()
    }
    pub async fn revoke_model_qualification(&self, binding: &ModelRegistryRef) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query("UPDATE model_registry_guard SET revision=revision WHERE id=1")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        if sqlx::query(
            "UPDATE model_registry SET revoked=1 WHERE id=$1 AND version=$2 AND bundle_sha256=$3",
        )
        .bind(&binding.id)
        .bind(i64::try_from(binding.version).map_err(storage_error)?)
        .bind(&binding.sha256)
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
            != 1
        {
            return Err(Error::NotFound);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
}
