use super::*;
use crate::library_config::{LibraryOwner, permit};
use cowork_contracts::{CodexProfileLocator, MaterializeCodexModelProfile, codex_profile_endpoint};
use cowork_identity::ContentAction;
impl Store {
    pub async fn original_codex_model_profile(
        &self,
        owner: LibraryOwner<'_>,
        account: &str,
        input: &MaterializeCodexModelProfile,
    ) -> Result<Option<ProviderProfile>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Read).await?;
        let row:Option<(String,String)>=sqlx::query_as("SELECT request_sha,receipt FROM codex_model_profile_operations WHERE scope=$1 AND operation_key=$2").bind(owner.namespace()).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let result = if let Some((sha, receipt)) = row {
            if sha != canonical_hash(&(account, input)) {
                return Err(Error::Conflict);
            }
            Some(parse(&receipt)?)
        } else {
            None
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn materialize_codex_model_profile(
        &self,
        owner: LibraryOwner<'_>,
        account: &str,
        input: &MaterializeCodexModelProfile,
    ) -> Result<ProviderProfile> {
        valid_text(&input.idempotency_key, 128)?;
        valid_text(&input.model, 200)?;
        if !(1024..=2_000_000).contains(&input.context_tokens) {
            return Err(Error::BudgetConfiguration);
        }
        let endpoint = codex_profile_endpoint(&CodexProfileLocator {
            account_id: account.into(),
            account_revision: input.expected_account_revision,
            reasoning_effort: input.reasoning_effort.clone(),
        })
        .ok_or(Error::Forbidden)?;
        let sha = canonical_hash(&(account, input));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::android_devices::device_writer(&mut tx).await?;
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        if let Some((old,receipt))=sqlx::query_as::<_,(String,String)>("SELECT request_sha,receipt FROM codex_model_profile_operations WHERE scope=$1 AND operation_key=$2").bind(owner.namespace()).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?{if old!=sha{return Err(Error::Conflict);}return parse(&receipt);}
        let raw:Option<String>=sqlx::query_scalar("SELECT payload FROM library_config_entries WHERE scope=$1 AND kind='codex_account' AND id=$2 AND deleted=0").bind(owner.namespace()).bind(account).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let account_value: CodexAccount = parse(&raw.ok_or(Error::Forbidden)?)?;
        if account_value.revision != input.expected_account_revision {
            return Err(Error::Conflict);
        }
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM codex_model_profiles WHERE scope=$1")
                .bind(owner.namespace())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if count >= 1000 {
            return Err(Error::BudgetExceeded);
        }
        let mut name = format!("{} · {}", account_value.value.name, input.model);
        while name.len() > 100 {
            name.pop();
        }
        let profile = ProviderProfile {
            id: id(),
            name,
            kind: ProviderKind::Codex,
            endpoint,
            model: input.model.clone(),
            context_tokens: input.context_tokens,
            revision: 1,
            credential_required: false,
            credential_ready: true,
            // Codex models read images through the Responses API.
            generation: Default::default(),
            capabilities: ModelCapabilities { image_input: true },
        };
        match owner {
            LibraryOwner::Local => {
                sqlx::query("INSERT INTO providers(id,payload) VALUES($1,$2)")
                    .bind(&profile.id)
                    .bind(json_string(&profile)?)
                    .execute(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            }
            LibraryOwner::Actor(actor) => {
                let defaults = crate::android_personal_provider::normalized_defaults(
                    json!({"base_url":profile.endpoint,"model":profile.model,"context_tokens":profile.context_tokens,"auth_mode":"none"}),
                )?;
                let stamp = crate::android_devices::device_iso(now())?;
                let value = json!({"schema_version":2,"id":profile.id,"revision":1,"etag":format!("{}:1",profile.id),"owner_user_id":actor.identity_id(),"team_id":null,"name":profile.name,"provider_kind":"codex","model_defaults":defaults,"has_secret":false,"created_at":stamp,"updated_at":stamp,"deleted_at":null});
                sqlx::query("INSERT INTO android_personal_provider_profiles(id,owner_id,revision,payload,deleted) VALUES($1,$2,1,$3,0)").bind(&profile.id).bind(actor.identity_id().to_string()).bind(json_string(&value)?).execute(&mut *tx).await.map_err(storage_error)?;
                crate::android_personal_provider::versions::persist(
                    &mut tx,
                    actor.identity_id(),
                    Uuid::parse_str(&profile.id).map_err(storage_error)?,
                )
                .await?;
            }
        }
        sqlx::query("INSERT INTO codex_model_profiles(scope,profile_id,account_id,payload) VALUES($1,$2,$3,$4)").bind(owner.namespace()).bind(&profile.id).bind(account).bind(json_string(&profile)?).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("INSERT INTO codex_model_profile_operations(scope,operation_key,request_sha,receipt) VALUES($1,$2,$3,$4)").bind(owner.namespace()).bind(&input.idempotency_key).bind(sha).bind(json_string(&profile)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(profile)
    }
    /// Trusted worker resolution. Namespace comes from the admitted owner, never model input.
    pub async fn codex_model_profile_account(
        &self,
        namespace: &str,
        profile: &ProviderProfile,
    ) -> Result<CodexProfileLocator> {
        let locator = cowork_contracts::parse_codex_profile_endpoint(&profile.endpoint)
            .ok_or(Error::Forbidden)?;
        let row:Option<(String,String)>=sqlx::query_as("SELECT m.payload,m.account_id FROM codex_model_profiles m JOIN library_config_entries a ON a.scope=m.scope AND a.kind='codex_account' AND a.id=m.account_id AND a.deleted=0 WHERE m.scope=$1 AND m.profile_id=$2").bind(namespace).bind(&profile.id).fetch_optional(&self.pool).await.map_err(storage_error)?;
        let (raw, account) = row.ok_or(Error::Forbidden)?;
        let expected: ProviderProfile = parse(&raw)?;
        if account != locator.account_id || canonical_hash(&expected) != canonical_hash(profile) {
            return Err(Error::Forbidden);
        }
        if namespace == "installation-owner" && self.provider_retired(&profile.id).await? {
            return Err(Error::Forbidden);
        }
        if let Some(owner) = namespace.strip_prefix("actor:") {
            let owner = Uuid::parse_str(owner).map_err(|_| Error::Forbidden)?;
            let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_provider_profiles WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(&profile.id).bind(owner.to_string()).fetch_one(&self.pool).await.map_err(storage_error)?;
            if active != 1 {
                return Err(Error::Forbidden);
            }
        } else if namespace != "installation-owner" {
            return Err(Error::Forbidden);
        }
        Ok(locator)
    }
}
