//! Private owner model keys bound to exact immutable provider revisions.
use super::*;
use cowork_secrets::{KeyId, KeyStore, open_bearer, seal_bearer};
use cowork_storage::personal_provider_credentials::{
    PersonalProviderCredential, personal_provider_scope,
};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct PersonalProviderKeys {
    pub keys: Arc<dyn KeyStore>,
    pub key_id: KeyId,
}
impl Runtime {
    pub fn with_personal_provider_keys(mut self, keys: Arc<dyn KeyStore>, key_id: KeyId) -> Self {
        self.personal_provider_keys = Some(PersonalProviderKeys { keys, key_id });
        self
    }
    pub async fn configure_personal_provider(
        &self,
        input: &ConfigurePersonalProvider,
    ) -> Result<ProviderProfile> {
        valid_text(&input.idempotency_key, 128)?;
        if input.explicitly_allow_no_key == input.api_key.is_some()
            || matches!(input.profile.kind, ProviderKind::Test | ProviderKind::Codex)
        {
            return Err(Error::Validation(
                "Choose a fresh API key or explicitly authorize anonymous access".into(),
            ));
        }
        let access = self
            .personal_provider_keys
            .as_ref()
            .ok_or(Error::NativeKeyUnavailable)?;
        if let Some(key) = &input.api_key {
            cowork_secrets::validate_bearer(key)
                .map_err(|_| Error::Validation("Invalid model API key".into()))?;
        }
        let profile = ProviderProfile {
            generation: Default::default(),
            id: id(),
            name: input.profile.name.trim().into(),
            kind: input.profile.kind.clone(),
            endpoint: input.profile.endpoint.trim_end_matches('/').into(),
            model: input.profile.model.trim().into(),
            context_tokens: input.profile.context_tokens,
            revision: 1,
            credential_required: input.api_key.is_some(),
            credential_ready: true,
            capabilities: input.profile.capabilities.clone(),
        };
        provider::validate_profile(&profile, false)?;
        let bytes = Zeroizing::new(
            serde_json::to_vec(&(
                "owner-personal-provider-request/v1",
                &input.profile,
                &input.api_key,
                input.explicitly_allow_no_key,
            ))
            .map_err(storage_error)?,
        );
        let master = access
            .keys
            .load(
                &access
                    .keys
                    .key_for_purpose(
                        &access.key_id,
                        cowork_secrets::KeyPurpose::RequestFingerprint,
                    )
                    .map_err(|_| Error::NativeKeyUnavailable)?,
            )
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let signature = ring::hmac::sign(
            &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, master.as_ref()),
            bytes.as_slice(),
        );
        let request_sha256: String = signature
            .as_ref()
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect();
        let scope = personal_provider_scope(&profile);
        let sealed = input
            .api_key
            .as_ref()
            .map(|key| seal_bearer(access.keys.as_ref(), access.key_id, &scope, key))
            .transpose()
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let credential = PersonalProviderCredential {
            profile_sha256: canonical_hash(&profile),
            scope,
            sealed,
            allow_no_key: input.explicitly_allow_no_key,
        };
        // The write guard prevents queue acceptance from observing a new profile
        // before its exact immutable profile/ciphertext transaction commits.
        let mut configured = self.providers.write().await;
        let accepted = self
            .store
            .create_personal_provider_with_credential(
                &input.idempotency_key,
                &request_sha256,
                &profile,
                &credential,
            )
            .await?;
        if !configured.iter().any(|p| p.profile.id == accepted.id)
            && !self.store.provider_retired(&accepted.id).await?
        {
            let current = self
                .store
                .providers()
                .await?
                .into_iter()
                .find(|p| p.id == accepted.id)
                .ok_or(Error::NotFound)?;
            configured.push(ConfiguredProvider {
                profile: current,
                credential_env: None,
            });
        }
        Ok(accepted)
    }
    pub async fn personal_provider_credential(
        &self,
        profile: &ProviderProfile,
    ) -> Result<Option<String>> {
        if let Some(record) = self.store.personal_provider_credential(profile).await? {
            return self.open_personal_provider_credential(record);
        }
        let configured = self
            .providers
            .read()
            .await
            .iter()
            .find(|p| p.profile.id == profile.id)
            .cloned()
            .ok_or_else(|| Error::Provider("profile_missing".into()))?;
        if canonical_hash(&configured.profile) != canonical_hash(profile) {
            return Err(Error::Conflict);
        }
        Ok(configured
            .credential_env
            .as_ref()
            .and_then(|name| std::env::var(name).ok())
            .filter(|value| !value.is_empty()))
    }
    pub(super) async fn personal_provider_credential_from(
        &self,
        configured: &ConfiguredProvider,
    ) -> Result<Option<String>> {
        if let Some(record) = self
            .store
            .personal_provider_credential(&configured.profile)
            .await?
        {
            return self.open_personal_provider_credential(record);
        }
        // Existing operator-configured environment profiles retain their original
        // lookup behavior. New browser/native profiles never select environment names.
        Ok(configured
            .credential_env
            .as_ref()
            .and_then(|name| std::env::var(name).ok())
            .filter(|value| !value.is_empty()))
    }

    fn open_personal_provider_credential(
        &self,
        record: PersonalProviderCredential,
    ) -> Result<Option<String>> {
        if record.allow_no_key {
            return Ok(None);
        }
        let access = self
            .personal_provider_keys
            .as_ref()
            .ok_or(Error::NativeKeyUnavailable)?;
        let bearer = open_bearer(
            access.keys.as_ref(),
            &record.scope,
            record.sealed.as_ref().ok_or(Error::Forbidden)?,
        )
        .map_err(|_| Error::NativeKeyUnavailable)?;
        Ok(Some(bearer.as_str().to_owned()))
    }
}
