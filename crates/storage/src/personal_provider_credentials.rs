//! Owner provider metadata and purpose-bound ciphertext commit together.
use super::*;
use cowork_secrets::{CredentialScope, SealedCredential};
use serde::{Deserialize, Serialize};
use sqlx::{Any, Transaction};

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

/// Profiles are immutable execution inputs. Ciphertext may only change through
/// key rewrapping or explicit revocation; a newer profile never replaces it.
pub(super) async fn persist_version(
    tx: &mut Transaction<'_, Any>,
    profile: &ProviderProfile,
    credential: &PersonalProviderCredential,
) -> Result<()> {
    credential.validate_for(profile)?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM personal_provider_credential_versions")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    if count >= 100_000 {
        return Err(Error::BudgetExceeded);
    }
    sqlx::query("INSERT INTO personal_provider_credential_versions(provider_id,revision,profile,payload) VALUES($1,$2,$3,$4)")
        .bind(&profile.id).bind(profile.revision).bind(json_string(profile)?).bind(json_string(credential)?)
        .execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}

pub(super) async fn backfill_versions(tx: &mut Transaction<'_, Any>) -> Result<()> {
    let rows=sqlx::query("SELECT p.payload AS profile,c.payload AS credential FROM personal_provider_credentials c JOIN providers p ON p.id=c.provider_id ORDER BY p.id")
        .fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 100_000 {
        return Err(Error::BudgetExceeded);
    }
    for row in rows {
        let profile: ProviderProfile =
            parse(&row.try_get::<String, _>("profile").map_err(storage_error)?)?;
        let credential: PersonalProviderCredential = parse(
            &row.try_get::<String, _>("credential")
                .map_err(storage_error)?,
        )?;
        persist_version(tx, &profile, &credential).await?;
    }
    Ok(())
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
        if let Some(sealed) = &credential.sealed {
            super::server_keys::cipher_writer(&mut tx, sealed.key_id()).await?;
        }
        if sqlx::query("UPDATE personal_provider_guard SET id=id WHERE id=1")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
            != 1
        {
            return Err(Error::Forbidden);
        }
        if let Some(row) = sqlx::query("SELECT r.request_sha256,v.profile AS profile FROM personal_provider_requests r LEFT JOIN personal_provider_credential_versions v ON v.provider_id=r.provider_id AND v.revision=1 WHERE r.request_key=$1")
            .bind(request_key).fetch_optional(&mut *tx).await.map_err(storage_error)?
        {
            if row.try_get::<String,_>("request_sha256").map_err(storage_error)? != request_sha256 {
                return Err(Error::Conflict);
            }
            let original=row.try_get::<Option<String>,_>("profile").map_err(storage_error)?.ok_or(Error::Conflict)?;
            let stored: ProviderProfile = parse(&original)?;
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
        persist_version(&mut tx, profile, credential).await?;
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
        if self.provider_retired(&profile.id).await? {
            return Err(Error::Forbidden);
        }
        let row = sqlx::query("SELECT profile,payload AS credential FROM personal_provider_credential_versions WHERE provider_id=$1 AND revision=$2")
            .bind(&profile.id).bind(profile.revision).fetch_optional(&self.pool).await.map_err(storage_error)?;
        let Some(row) = row else {
            let managed: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM personal_provider_credential_versions WHERE provider_id=$1",
            )
            .bind(&profile.id)
            .fetch_one(&self.pool)
            .await
            .map_err(storage_error)?;
            if managed != 0 {
                return Err(Error::Conflict);
            }
            return Ok(None);
        };
        let stored: ProviderProfile =
            parse(&row.try_get::<String, _>("profile").map_err(storage_error)?)?;
        if canonical_hash(&stored) != canonical_hash(profile) {
            return Err(Error::Conflict);
        }
        let payload = row
            .try_get::<Option<String>, _>("credential")
            .map_err(storage_error)?
            .ok_or(Error::Forbidden)?;
        let credential: PersonalProviderCredential = parse(&payload)?;
        credential.validate_for(profile)?;
        Ok(Some(credential))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migration_backfills_existing_exact_bound_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider-upgrade.sqlite");
        sqlx::any::install_default_drivers();
        let encoded = percent_encoding::utf8_percent_encode(
            path.to_str().unwrap(),
            percent_encoding::NON_ALPHANUMERIC,
        );
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite:{encoded}?mode=rwc"))
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        cowork_identity::migrations::apply(&mut tx).await.unwrap();
        crate::tenant_projects::migrate(&mut tx).await.unwrap();
        crate::team_service::migrate(&mut tx).await.unwrap();
        crate::team_service::migrate_integrations(&mut tx)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE schema_migrations(version BIGINT PRIMARY KEY,checksum TEXT NOT NULL)",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        // Construct a genuine prefix instead of rewinding the latest schema;
        // later additive migrations must not leave gaps in the upgrade fixture.
        for (version, sql) in crate::migrations::MIGRATIONS
            .iter()
            .take_while(|(version, _)| *version < 80)
        {
            for statement in sql
                .split(';')
                .map(str::trim)
                .filter(|statement| !statement.is_empty())
            {
                sqlx::query(statement).execute(&mut *tx).await.unwrap();
            }
            sqlx::query("INSERT INTO schema_migrations(version,checksum) VALUES($1,$2)")
                .bind(version)
                .bind(hash(sql.as_bytes()))
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        let profile = ProviderProfile {
            id: Uuid::new_v4().to_string(),
            name: "Existing anonymous model".into(),
            kind: ProviderKind::OpenaiCompatible,
            endpoint: "https://fixture.invalid/v1".into(),
            model: "fixture".into(),
            revision: 1,
            context_tokens: 32768,
            credential_required: false,
            credential_ready: true,
            generation: Default::default(),
            capabilities: Default::default(),
        };
        let credential = PersonalProviderCredential {
            profile_sha256: canonical_hash(&profile),
            scope: personal_provider_scope(&profile),
            sealed: None,
            allow_no_key: true,
        };
        sqlx::query("INSERT INTO providers(id,payload) VALUES($1,$2)")
            .bind(&profile.id)
            .bind(json_string(&profile).unwrap())
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO personal_provider_credentials(provider_id,payload) VALUES($1,$2)")
            .bind(&profile.id)
            .bind(json_string(&credential).unwrap())
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO personal_provider_requests(request_key,request_sha256,provider_id,created_at) VALUES('upgrade-fixture',$1,$2,0)").bind("a".repeat(64)).bind(&profile.id).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        pool.close().await;
        let upgraded = Store::sqlite(&path).await.unwrap();
        let actual = upgraded
            .personal_provider_credential(&profile)
            .await
            .unwrap()
            .unwrap();
        actual.validate_for(&profile).unwrap();
        assert!(actual.allow_no_key);
        assert_eq!(actual.profile_sha256, canonical_hash(&profile));
        assert_eq!(
            upgraded
                .create_personal_provider_with_credential(
                    "upgrade-fixture",
                    &"a".repeat(64),
                    &profile,
                    &credential
                )
                .await
                .unwrap()
                .revision,
            1
        );
    }
}
