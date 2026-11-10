//! Native operator qualifications for actual actor-owned private project/profile tuples.
//! This namespace never reads or creates owner `providers` or global registry rows.
use super::*;
use crate::android_devices::device_writer;
use crate::model_registry::{accounting, view};
use cowork_identity::{ContentAction, Principal};
use cowork_model_accounting::{
    registry::{QualificationBundle, RawWireMode, RegisteredModel},
    tokenizer::TokenCount,
};
use sqlx::{Any, Transaction};

fn invalid() -> Error {
    Error::Validation("actor_model_registry_invalid".into())
}
fn uuid(value: &str) -> Result<Uuid> {
    Uuid::parse_str(value)
        .ok()
        .filter(|id| !id.is_nil() && id.to_string() == value)
        .ok_or_else(invalid)
}
fn sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn reference(value: &ModelRegistryRef) -> Result<()> {
    if value.id.is_empty()
        || value.id.len() > 512
        || value.id.chars().any(char::is_control)
        || value.version == 0
        || value.version > i64::MAX as u64
        || !sha(&value.sha256)
    {
        return Err(invalid());
    }
    Ok(())
}

/// The one neutral projection used by admission, worker wire preparation and native import.
/// It contains metadata only and is bound separately to the complete actor profile revision.
pub fn provider_profile(profile: &Value) -> Result<ProviderProfile> {
    if (profile["provider_kind"] != "openai_compatible" && profile["provider_kind"] != "codex")
        || !profile["team_id"].is_null()
        || !profile["deleted_at"].is_null()
    {
        return Err(Error::Forbidden);
    }
    let id = uuid(profile["id"].as_str().ok_or_else(invalid)?)?;
    uuid(profile["owner_user_id"].as_str().ok_or_else(invalid)?)?;
    let revision = profile["revision"]
        .as_i64()
        .filter(|v| *v > 0)
        .ok_or_else(invalid)?;
    let defaults =
        crate::android_personal_provider::normalized_defaults(profile["model_defaults"].clone())?;
    if defaults != profile["model_defaults"] || !profile["has_secret"].is_boolean() {
        return Err(invalid());
    }
    let auth = defaults["auth_mode"].as_str().ok_or_else(invalid)?;
    let has_secret = profile["has_secret"].as_bool().ok_or_else(invalid)?;
    if auth != "none" && !has_secret {
        return Err(Error::Forbidden);
    }
    let name = profile["name"]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 200 && !v.chars().any(char::is_control))
        .ok_or_else(invalid)?;
    Ok(ProviderProfile {
        // This projection is part of existing qualification hashes. Actor
        // generation settings remain in the separately revision-bound profile
        // and freeze into RunSpec.agent_config at execution admission.
        generation: Default::default(),
        id: id.to_string(),
        name: name.into(),
        kind: if profile["provider_kind"] == "codex" {
            ProviderKind::Codex
        } else {
            ProviderKind::OpenaiCompatible
        },
        endpoint: defaults["base_url"].as_str().ok_or_else(invalid)?.into(),
        model: defaults["model"].as_str().ok_or_else(invalid)?.into(),
        revision,
        context_tokens: u32::try_from(defaults["context_tokens"].as_u64().ok_or_else(invalid)?)
            .map_err(storage_error)?,
        credential_required: has_secret && auth != "none",
        credential_ready: true,
        capabilities: ModelCapabilities {
            image_input: profile["provider_kind"] == "codex",
        },
    })
}

async fn private_project(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
) -> Result<Value> {
    crate::android_private_sharing::stamp(tx, owner, project, ContentAction::Execute).await?;
    let row=sqlx::query("SELECT p.payload,p.revision,p.owner_id FROM android_personal_projects p JOIN identity_subjects i ON i.id=p.owner_id WHERE p.id=$1 AND p.deleted=0 AND i.active=1")
        .bind(project.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let row = row.ok_or(Error::Forbidden)?;
    let value: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if value["id"] != json!(project)
        || value["owner_user_id"]
            != json!(
                row.try_get::<String, _>("owner_id")
                    .map_err(storage_error)?
            )
        || value["privacy"] != "private_local"
        || !value["team_id"].is_null()
        || value["revision"].as_i64().is_none_or(|v| v <= 0)
        || value["revision"] != json!(row.try_get::<i64, _>("revision").map_err(storage_error)?)
    {
        return Err(Error::Forbidden);
    }
    Ok(value)
}
async fn actual_profile(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    profile: &Value,
) -> Result<ProviderProfile> {
    if profile["owner_user_id"] != json!(owner) {
        return Err(Error::Forbidden);
    }
    let id = uuid(profile["id"].as_str().ok_or_else(invalid)?)?;
    let current = crate::android_personal_provider::versions::frozen(
        tx,
        owner,
        &crate::android_personal_provider::binding(profile)?,
    )
    .await?;
    if current != *profile || current["id"] != json!(id) {
        return Err(Error::Forbidden);
    }
    provider_profile(&current)
}
async fn context(
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    project: Uuid,
    profile: Uuid,
    store: &Store,
    action: ContentAction,
) -> Result<ActorModelContext> {
    let p = store
        .android_personal_permit(tx, actor, project, action)
        .await?;
    private_project(tx, actor.identity_id(), project).await?;
    let value = crate::android_personal_provider::current(tx, actor.identity_id(), profile).await?;
    let provider = actual_profile(tx, actor.identity_id(), &value).await?;
    Ok(ActorModelContext {
        actor_id: actor.identity_id().to_string(),
        project_id: project.to_string(),
        project_revision: p["revision"]
            .as_i64()
            .filter(|v| *v > 0)
            .ok_or(Error::Forbidden)?,
        provider_binding: crate::android_personal_provider::binding(&value)?,
        provider,
    })
}
fn compatible(model: &RegisteredModel, provider: &ProviderProfile) -> Result<()> {
    let bundle = &model.bundle;
    if bundle.id.len() > 128
        || bundle.provider_sha256 != canonical_hash(provider)
        || bundle.qualification.model.provider_id != provider.id
        || bundle.qualification.model.model_id != provider.model
        || bundle.qualification.model.context_tokens != u64::from(provider.context_tokens)
        || !matches!(bundle.wire_mode, RawWireMode::OpenaiCompletions)
        || !matches!(provider.kind, ProviderKind::OpenaiCompatible)
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

/// A run JSON can select an already admitted immutable reference, never create trust.
/// Called inside the original actor/project/session/lease transaction by the Android worker.
pub(crate) async fn check_reference(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    profile: &Value,
    optional_model_binding: &Value,
) -> Result<Option<RegisteredModel>> {
    let current_project = private_project(tx, owner, project).await?;
    let provider = actual_profile(tx, owner, profile).await?;
    if optional_model_binding.is_null() {
        return Ok(None);
    }
    let binding: ModelRegistryRef =
        serde_json::from_value(optional_model_binding.clone()).map_err(|_| invalid())?;
    reference(&binding)?;
    // This fence shares lock order with native import/revoke. Revoke cannot commit
    // between model admission and the dispatch fence using this transaction.
    sqlx::query("UPDATE actor_model_registry_guard SET revision=revision WHERE id=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    let row=sqlx::query("SELECT * FROM actor_model_registry WHERE owner_id=$1 AND project_id=$2 AND id=$3 AND version=$4")
        .bind(owner.to_string()).bind(project.to_string()).bind(&binding.id).bind(binding.version as i64)
        .fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let receipt: ActorModelRegistryView =
        parse(&row.try_get::<String, _>("receipt").map_err(storage_error)?)?;
    let profile_binding = crate::android_personal_provider::binding(profile)?;
    if row.try_get::<i64, _>("revoked").map_err(storage_error)? != 0
        || row
            .try_get::<String, _>("provider_id")
            .map_err(storage_error)?
            != provider.id
        || row
            .try_get::<i64, _>("provider_revision")
            .map_err(storage_error)?
            != provider.revision
        || row
            .try_get::<String, _>("provider_binding_sha256")
            .map_err(storage_error)?
            != canonical_hash(&profile_binding)
        || row
            .try_get::<String, _>("provider_sha256")
            .map_err(storage_error)?
            != canonical_hash(&provider)
        || row
            .try_get::<String, _>("bundle_sha256")
            .map_err(storage_error)?
            != binding.sha256
        || canonical_hash(&receipt.qualification.binding) != canonical_hash(&binding)
        || receipt.context.actor_id != owner.to_string()
        || receipt.context.project_id != project.to_string()
        || json!(receipt.context.project_revision) != current_project["revision"]
        || receipt.context.provider_binding != profile_binding
        || canonical_hash(&receipt.context.provider) != canonical_hash(&provider)
        || !sha(&receipt.source_sha256)
        || !sha(&receipt.request_sha256)
    {
        return Err(Error::Forbidden);
    }
    let bundle: QualificationBundle =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    let model = RegisteredModel::load(bundle).map_err(accounting)?;
    if model.sha256().map_err(accounting)? != binding.sha256 {
        return Err(Error::Forbidden);
    }
    compatible(&model, &provider)?;
    Ok(Some(model))
}
pub(crate) async fn check_binding(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    profile: &Value,
    spec: &RunSpec,
) -> Result<Option<RegisteredModel>> {
    let current_project = private_project(tx, owner, project).await?;
    if spec.project_id != project.to_string()
        || json!(spec.policy_revision) != current_project["revision"]
        || canonical_hash(spec.provider.as_ref().ok_or(Error::Forbidden)?)
            != canonical_hash(&provider_profile(profile)?)
    {
        return Err(Error::Forbidden);
    }
    check_reference(
        tx,
        owner,
        project,
        profile,
        &serde_json::to_value(&spec.model_binding).map_err(storage_error)?,
    )
    .await
}
pub fn validate_context(
    model: &RegisteredModel,
    spec: &RunSpec,
    messages: &[Value],
    logical: &str,
) -> Result<TokenCount> {
    let tools = model_tool_catalog(spec);
    let binding = spec.model_binding.as_ref().ok_or(Error::Forbidden)?;
    let provider = spec.provider.as_ref().ok_or(Error::Forbidden)?;
    if model.sha256().map_err(accounting)? != binding.sha256
        || model.bundle.id != binding.id
        || model.bundle.version != binding.version
        || model_request_fingerprint(spec, messages, &tools)? != logical
    {
        return Err(Error::Forbidden);
    }
    compatible(model, provider)?;
    let prompt = model
        .prompt(
            messages,
            &tools,
            &serde_json::to_value(&spec.output).map_err(storage_error)?,
            logical,
        )
        .map_err(accounting)?;
    model
        .tokenizer
        .count(&prompt, u64::from(spec.budgets.max_output_tokens))
        .map_err(accounting)
}

impl Store {
    pub async fn actor_model_registry_authority(
        &self,
        actor: &Principal,
        project: Uuid,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = self
            .android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        private_project(&mut tx, actor.identity_id(), project).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"actor_id":actor.identity_id(),"project_id":project,"project_revision":value["revision"]}),
        )
    }
    pub async fn actor_model_context(
        &self,
        actor: &Principal,
        project: Uuid,
        profile: Uuid,
    ) -> Result<ActorModelContext> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = context(&mut tx, actor, project, profile, self, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn import_actor_model_qualification(
        &self,
        actor: &Principal,
        input: &ImportActorModelQualification,
    ) -> Result<ActorModelRegistryView> {
        uuid(&input.idempotency_key)?;
        let project = uuid(&input.context.project_id)?;
        let profile = uuid(&input.context.provider.id)?;
        if input.context.actor_id != actor.identity_id().to_string()
            || !sha(&input.source_sha256)
            || serde_json::to_vec(input).map_err(storage_error)?.len() > 63 * 1024 * 1024
        {
            return Err(invalid());
        }
        let request = canonical_hash(input);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        sqlx::query("UPDATE actor_model_registry_guard SET revision=revision WHERE id=1")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        // Current actor/project/session authority is required even to retrieve a
        // receipt. Profile edits and later revocation do not rewrite that receipt.
        if let Some(row)=sqlx::query("SELECT project_id,request_sha256,receipt FROM actor_model_registry_operations WHERE owner_id=$1 AND idempotency_key=$2")
            .bind(actor.identity_id().to_string()).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)? {
            if row.try_get::<String,_>("project_id").map_err(storage_error)?!=project.to_string()
                || row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=request { return Err(Error::Conflict); }
            let receipt:ActorModelRegistryView=parse(&row.try_get::<String,_>("receipt").map_err(storage_error)?)?;
            if receipt.request_sha256!=request || canonical_hash(&receipt.context)!=canonical_hash(&input.context)
                || receipt.source_sha256!=input.source_sha256 { return Err(Error::Forbidden); }
            tx.commit().await.map_err(storage_error)?;
            return Ok(receipt);
        }
        let current = context(
            &mut tx,
            actor,
            project,
            profile,
            self,
            ContentAction::Execute,
        )
        .await?;
        if canonical_hash(&current) != canonical_hash(&input.context) {
            return Err(Error::Conflict);
        }
        let bundle: QualificationBundle =
            serde_json::from_value(input.qualification_bundle.clone()).map_err(|_| invalid())?;
        let model = RegisteredModel::load(bundle).map_err(accounting)?;
        compatible(&model, &current.provider)?;
        let bundle = &model.bundle;
        let highest:Option<i64>=sqlx::query_scalar("SELECT MAX(version) FROM actor_model_registry WHERE owner_id=$1 AND project_id=$2 AND id=$3")
            .bind(&current.actor_id).bind(&current.project_id).bind(&bundle.id).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if highest.unwrap_or(0) >= i64::try_from(bundle.version).map_err(storage_error)? {
            return Err(Error::Conflict);
        }
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM actor_model_registry WHERE owner_id=$1")
                .bind(&current.actor_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        let ops: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM actor_model_registry_operations WHERE owner_id=$1",
        )
        .bind(&current.actor_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let total: i64 =
            sqlx::query_scalar("SELECT COALESCE(SUM(LENGTH(payload)),0) FROM actor_model_registry")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        let payload = json_string(bundle)?;
        if count >= 64
            || ops >= 1024
            || total.saturating_add(payload.len() as i64) > 512 * 1024 * 1024
        {
            return Err(Error::Validation(
                "actor_model_registry_retention_limit".into(),
            ));
        }
        let time = now();
        let mut qualification = view(bundle, time, false)?;
        qualification.provider_revision = current.provider.revision;
        let receipt = ActorModelRegistryView {
            context: current.clone(),
            qualification,
            source_sha256: input.source_sha256.clone(),
            request_sha256: request.clone(),
        };
        sqlx::query("INSERT INTO actor_model_registry(owner_id,project_id,id,version,provider_id,provider_revision,provider_binding_sha256,provider_sha256,bundle_sha256,payload,receipt,registered_at,revoked) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,0)")
            .bind(&current.actor_id).bind(&current.project_id).bind(&bundle.id).bind(bundle.version as i64).bind(&current.provider.id).bind(current.provider.revision)
            .bind(canonical_hash(&current.provider_binding)).bind(canonical_hash(&current.provider)).bind(&receipt.qualification.binding.sha256)
            .bind(payload).bind(json_string(&receipt)?).bind(time).execute(&mut *tx).await.map_err(storage_error)?;
        let profile_value =
            crate::android_personal_provider::current(&mut tx, actor.identity_id(), profile)
                .await?;
        check_reference(
            &mut tx,
            actor.identity_id(),
            project,
            &profile_value,
            &serde_json::to_value(&receipt.qualification.binding).map_err(storage_error)?,
        )
        .await?;
        sqlx::query("INSERT INTO actor_model_registry_operations(owner_id,project_id,idempotency_key,request_sha256,receipt) VALUES($1,$2,$3,$4,$5)")
            .bind(&current.actor_id).bind(&current.project_id).bind(&input.idempotency_key).bind(request).bind(json_string(&receipt)?).execute(&mut *tx).await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn actor_model_registry(
        &self,
        actor: &Principal,
        project: Uuid,
    ) -> Result<Vec<ActorModelRegistryView>> {
        let mut tx = self.snapshot_transaction().await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        let current_project = private_project(&mut tx, actor.identity_id(), project).await?;
        let rows=sqlx::query("SELECT receipt,bundle_sha256,provider_binding_sha256,provider_sha256,provider_id,provider_revision,revoked FROM actor_model_registry WHERE owner_id=$1 AND project_id=$2 ORDER BY id,version LIMIT 64")
            .bind(actor.identity_id().to_string()).bind(project.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let mut result = Vec::new();
        for row in rows {
            let mut entry: ActorModelRegistryView =
                parse(&row.try_get::<String, _>("receipt").map_err(storage_error)?)?;
            if entry.context.actor_id != actor.identity_id().to_string()
                || entry.context.project_id != project.to_string()
                || entry.qualification.binding.sha256
                    != row
                        .try_get::<String, _>("bundle_sha256")
                        .map_err(storage_error)?
                || canonical_hash(&entry.context.provider_binding)
                    != row
                        .try_get::<String, _>("provider_binding_sha256")
                        .map_err(storage_error)?
                || canonical_hash(&entry.context.provider)
                    != row
                        .try_get::<String, _>("provider_sha256")
                        .map_err(storage_error)?
                || entry.context.provider.id
                    != row
                        .try_get::<String, _>("provider_id")
                        .map_err(storage_error)?
                || entry.context.provider.revision
                    != row
                        .try_get::<i64, _>("provider_revision")
                        .map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
            let current = crate::android_personal_provider::current(
                &mut tx,
                actor.identity_id(),
                uuid(&entry.context.provider.id)?,
            )
            .await;
            let matches = match current {
                Ok(profile) => {
                    crate::android_personal_provider::binding(&profile)?
                        == entry.context.provider_binding
                        && provider_profile(&profile).is_ok_and(|p| {
                            canonical_hash(&p) == canonical_hash(&entry.context.provider)
                        })
                }
                Err(Error::NotFound) => false,
                Err(e) => return Err(e),
            };
            entry.qualification.revoked =
                row.try_get::<i64, _>("revoked").map_err(storage_error)? != 0
                    || !matches
                    || json!(entry.context.project_revision) != current_project["revision"];
            result.push(entry);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn revoke_actor_model_qualification(
        &self,
        actor: &Principal,
        input: &RevokeActorModelQualification,
    ) -> Result<()> {
        if input.actor_id != actor.identity_id().to_string() {
            return Err(Error::Forbidden);
        }
        let project = uuid(&input.project_id)?;
        reference(&input.binding)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        sqlx::query("UPDATE actor_model_registry_guard SET revision=revision WHERE id=1")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        if sqlx::query("UPDATE actor_model_registry SET revoked=1 WHERE owner_id=$1 AND project_id=$2 AND id=$3 AND version=$4 AND bundle_sha256=$5")
            .bind(&input.actor_id).bind(&input.project_id).bind(&input.binding.id).bind(input.binding.version as i64).bind(&input.binding.sha256)
            .execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1 { return Err(Error::NotFound); }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_registered_model(
        &self,
        run: Uuid,
        lease: Uuid,
        spec: &RunSpec,
    ) -> Result<Option<RegisteredModel>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let row = crate::android_private_server::leased(self, &mut tx, run, lease, current).await?;
        let owner = crate::android_private_server::id(&row, "owner_id")?;
        let project = crate::android_private_server::id(&row, "project_id")?;
        let frozen: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let profile = crate::android_personal_provider::versions::frozen(
            &mut tx,
            owner,
            &frozen["spec"]["input"]["provider_binding"],
        )
        .await?;
        if frozen["spec"]["input"]["model_binding"]
            != serde_json::to_value(&spec.model_binding).map_err(storage_error)?
            || spec.order_id != crate::android_private_server::id(&row, "thread_id")?.to_string()
            || spec.policy_revision
                != row
                    .try_get::<i64, _>("project_revision")
                    .map_err(storage_error)?
            || spec.executor != "android_private_server_linux"
        {
            return Err(Error::Forbidden);
        }
        let model = check_binding(&mut tx, owner, project, &profile, spec).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(model)
    }
}
