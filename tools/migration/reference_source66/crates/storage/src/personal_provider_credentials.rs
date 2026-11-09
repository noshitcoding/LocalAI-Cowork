//! Owner provider metadata and purpose-bound ciphertext commit together.
use super::*;
use cowork_secrets::{CredentialScope, SealedCredential};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonalProviderCredential {
    pub profile_sha256: String,
    pub scope: CredentialScope,
    pub sealed: Option<SealedCredential>,
    pub allow_no_key: bool,
}
pub fn personal_provider_scope(profile: &ProviderProfile) -> CredentialScope {
    CredentialScope {
        project_id: "owner-personal-providers/v1".into(),
        server_id: profile.id.clone(),
        reference: "personal-model-key/v1".into(),
        revision: profile.revision,
        endpoint: profile.endpoint.clone(),
    }
}
impl PersonalProviderCredential {
    pub fn validate_for(&self, profile: &ProviderProfile) -> Result<()> {
        self.scope.validate().map_err(|_| Error::Forbidden)?;
        if self.profile_sha256 != canonical_hash(profile)
            || self.scope != personal_provider_scope(profile)
            || self.allow_no_key == self.sealed.is_some()
            || profile.credential_required != self.sealed.is_some()
            || !profile.credential_ready
            || self.sealed.as_ref().is_some_and(|v| v.validate().is_err())
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
impl Store {
    pub async fn create_personal_provider_with_credential(
        &self,
        request_key: &str,
        request_sha256: &str,
        profile: &ProviderProfile,
        credential: &PersonalProviderCredential,
    ) -> Result<ProviderProfile> {
        valid_text(request_key, 128)?;
        if request_sha256.len() != 64
            || !request_sha256.bytes().all(|v| v.is_ascii_hexdigit())
            || Uuid::parse_str(&profile.id).is_err()
            || profile.revision != 1
        {
            return Err(Error::Validation(
                "Invalid personal provider request".into(),
            ));
        }
        credential.validate_for(profile)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        if let Some(sealed)=&credential.sealed {super::server_keys::cipher_writer(&mut tx,sealed.key_id()).await?;}
        if sqlx::query("UPDATE personal_provider_guard SET id=id WHERE id=1")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
            != 1
        {
            return Err(Error::Forbidden);
        }
        if let Some(row) = sqlx::query("SELECT r.request_sha256,p.payload AS profile,c.payload AS credential FROM personal_provider_requests r JOIN providers p ON p.id=r.provider_id JOIN personal_provider_credentials c ON c.provider_id=p.id WHERE r.request_key=$1")
            .bind(request_key).fetch_optional(&mut *tx).await.map_err(storage_error)?
        {
            if row.try_get::<String,_>("request_sha256").map_err(storage_error)? != request_sha256 {
                return Err(Error::Conflict);
            }
            let stored: ProviderProfile = parse(&row.try_get::<String,_>("profile").map_err(storage_error)?)?;
            let bound: PersonalProviderCredential = parse(&row.try_get::<String,_>("credential").map_err(storage_error)?)?;
            bound.validate_for(&stored)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(stored);
        }
        sqlx::query("INSERT INTO providers(id,payload) VALUES($1,$2)")
            .bind(&profile.id)
            .bind(json_string(profile)?)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        sqlx::query("INSERT INTO personal_provider_credentials(provider_id,payload) VALUES($1,$2)")
            .bind(&profile.id)
            .bind(json_string(credential)?)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        sqlx::query("INSERT INTO personal_provider_requests(request_key,request_sha256,provider_id,created_at) VALUES($1,$2,$3,$4)")
            .bind(request_key).bind(request_sha256).bind(&profile.id).bind(now())
            .execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(profile.clone())
    }
    pub async fn personal_provider_credential(
        &self,
        profile: &ProviderProfile,
    ) -> Result<Option<PersonalProviderCredential>> {
        let row = sqlx::query("SELECT p.payload AS profile,c.payload AS credential FROM personal_provider_credentials c JOIN providers p ON p.id=c.provider_id WHERE c.provider_id=$1")
            .bind(&profile.id).fetch_optional(&self.pool).await.map_err(storage_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let stored: ProviderProfile =
            parse(&row.try_get::<String, _>("profile").map_err(storage_error)?)?;
        if canonical_hash(&stored) != canonical_hash(profile) {
            return Err(Error::Conflict);
        }
        let credential: PersonalProviderCredential = parse(
            &row.try_get::<String, _>("credential")
                .map_err(storage_error)?,
        )?;
        credential.validate_for(profile)?;
        Ok(Some(credential))
    }
}
