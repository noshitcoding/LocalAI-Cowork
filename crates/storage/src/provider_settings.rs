//! Revisioned owner configuration. Credentials and public profile commit together.
use super::*;
use crate::personal_provider_credentials::PersonalProviderCredential;

impl Store {
    pub async fn provider_generation(&self, id: &str) -> Result<GenerationSettings> {
        let row =
            sqlx::query("SELECT payload FROM provider_generation_settings WHERE provider_id=$1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_error)?;
        row.map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .transpose()
            .map(|v| v.unwrap_or_default())
    }

    pub async fn provider_retired(&self, id: &str) -> Result<bool> {
        Ok(
            sqlx::query("SELECT provider_id FROM provider_retirements WHERE provider_id=$1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_error)?
                .is_some(),
        )
    }

    pub async fn provider_settings_receipt(
        &self,
        key: &str,
        digest: &str,
    ) -> Result<Option<Value>> {
        let row = sqlx::query(
            "SELECT request_sha256,payload FROM provider_setting_requests WHERE request_key=$1",
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        let Some(row) = row else { return Ok(None) };
        if row
            .try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != digest
        {
            return Err(Error::Conflict);
        }
        Ok(Some(parse(
            &row.try_get::<String, _>("payload").map_err(storage_error)?,
        )?))
    }

    pub async fn write_provider_settings(
        &self,
        key: &str,
        digest: &str,
        expected: i64,
        profile: &ProviderProfile,
        generation: &GenerationSettings,
        credential: Option<&PersonalProviderCredential>,
    ) -> Result<ProviderSettings> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query("UPDATE personal_provider_guard SET id=id WHERE id=1")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        if let Some(row) = sqlx::query(
            "SELECT request_sha256,payload FROM provider_setting_requests WHERE request_key=$1",
        )
        .bind(key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        {
            if row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != digest
            {
                return Err(Error::Conflict);
            }
            return parse(&row.try_get::<String, _>("payload").map_err(storage_error)?);
        }
        let row = sqlx::query("SELECT payload FROM providers WHERE id=$1")
            .bind(&profile.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        let old: ProviderProfile =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if old.revision != expected || profile.revision != expected + 1 {
            return Err(Error::Conflict);
        }
        if matches!(profile.kind, ProviderKind::Test) {
            if credential.is_some() {
                return Err(Error::Forbidden);
            }
        } else {
            let anonymous = PersonalProviderCredential {
                profile_sha256: canonical_hash(profile),
                scope: super::personal_provider_credentials::personal_provider_scope(profile),
                sealed: None,
                allow_no_key: true,
            };
            let credential = credential.unwrap_or(&anonymous);
            credential.validate_for(profile)?;
            if let Some(sealed) = &credential.sealed {
                super::server_keys::cipher_writer(&mut tx, sealed.key_id()).await?;
            }
            sqlx::query("INSERT INTO personal_provider_credentials(provider_id,payload) VALUES($1,$2) ON CONFLICT(provider_id) DO UPDATE SET payload=excluded.payload")
                .bind(&profile.id).bind(json_string(credential)?).execute(&mut *tx).await.map_err(storage_error)?;
            super::personal_provider_credentials::persist_version(&mut tx, profile, credential)
                .await?;
        }
        sqlx::query("UPDATE providers SET payload=$1 WHERE id=$2")
            .bind(json_string(profile)?)
            .bind(&profile.id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        sqlx::query("INSERT INTO provider_generation_settings(provider_id,payload) VALUES($1,$2) ON CONFLICT(provider_id) DO UPDATE SET payload=excluded.payload")
            .bind(&profile.id).bind(json_string(generation)?).execute(&mut *tx).await.map_err(storage_error)?;
        let result = ProviderSettings {
            profile: profile.clone(),
            generation: generation.clone(),
        };
        sqlx::query("INSERT INTO provider_setting_requests(request_key,request_sha256,payload) VALUES($1,$2,$3)")
            .bind(key).bind(digest).bind(json_string(&result)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }

    pub async fn retire_provider(
        &self,
        provider: &str,
        input: &DeleteProviderSettings,
    ) -> Result<()> {
        valid_text(&input.idempotency_key, 128)?;
        let digest = canonical_hash(&json!(["delete-provider", provider, input]));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query("UPDATE personal_provider_guard SET id=id WHERE id=1")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        if let Some(row) =
            sqlx::query("SELECT request_sha256 FROM provider_setting_requests WHERE request_key=$1")
                .bind(&input.idempotency_key)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
        {
            return if row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                == digest
            {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        let retired: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM provider_retirements WHERE provider_id=$1")
                .bind(provider)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if retired != 0 {
            return Err(Error::NotFound);
        }
        let row = sqlx::query("SELECT payload FROM providers WHERE id=$1")
            .bind(provider)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        let old: ProviderProfile =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if old.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        // Frozen runs keep their original profile. Removing a connection never replays a request.
        sqlx::query("INSERT INTO provider_retirements(provider_id,retired_at) VALUES($1,$2)")
            .bind(provider)
            .bind(now())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        sqlx::query("DELETE FROM personal_provider_credentials WHERE provider_id=$1")
            .bind(provider)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        // Retirement revokes every retained key, while preserving immutable
        // metadata and original acknowledgements for history/recovery.
        sqlx::query(
            "UPDATE personal_provider_credential_versions SET payload=NULL WHERE provider_id=$1",
        )
        .bind(provider)
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        sqlx::query("INSERT INTO provider_setting_requests(request_key,request_sha256,payload) VALUES($1,$2,$3)")
            .bind(&input.idempotency_key).bind(digest).bind("{\"deleted\":true}").execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
}
