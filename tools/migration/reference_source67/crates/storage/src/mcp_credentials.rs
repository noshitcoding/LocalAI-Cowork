use super::*;
use cowork_domain::mcp::McpServerRecord;
use cowork_secrets::{CredentialScope, SealedCredential};
use serde::{Deserialize, Serialize};

/// Private encrypted record. Excluded from public order/run exports and events.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpCredentialRecord {
    pub scope: CredentialScope,
    pub sealed: Option<SealedCredential>,
    pub revision: i64,
    pub revoked: bool,
}
impl McpCredentialRecord {
    fn validate(&self) -> Result<()> {
        self.scope.validate().map_err(|_| Error::Forbidden)?;
        let endpoint =
            cowork_mcp::Endpoint::new(&self.scope.endpoint, None).map_err(|_| Error::Forbidden)?;
        if endpoint.url() != self.scope.endpoint {
            return Err(Error::Forbidden);
        }
        if self.revoked {
            if self.sealed.is_some() || self.scope.revision.checked_add(1) != Some(self.revision) {
                return Err(Error::Forbidden);
            }
        } else if self.revision != self.scope.revision
            || self.sealed.as_ref().is_none_or(|s| s.validate().is_err())
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
    pub fn sealed_for(&self, scope: &CredentialScope) -> Result<&SealedCredential> {
        self.validate()?;
        if self.revoked || &self.scope != scope {
            return Err(Error::Forbidden);
        }
        self.sealed.as_ref().ok_or(Error::Forbidden)
    }
}
pub fn scope_for(server: &McpServerRecord) -> Option<CredentialScope> {
    server
        .configuration
        .credential_reference
        .as_ref()
        .map(|reference| CredentialScope {
            project_id: server.project_id.clone(),
            server_id: server.id.clone(),
            reference: reference.clone(),
            revision: server.configuration.credential_revision,
            endpoint: server.configuration.endpoint.clone(),
        })
}
pub(super) fn checked(row: &sqlx::any::AnyRow) -> Result<McpCredentialRecord> {
    let record: McpCredentialRecord =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    record.validate()?;
    if record.scope.project_id
        != row
            .try_get::<String, _>("project_id")
            .map_err(storage_error)?
        || record.scope.reference
            != row
                .try_get::<String, _>("reference")
                .map_err(storage_error)?
        || record.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(record)
}
impl Store {
    /// Native/control-plane operation. Caller retains ciphertext for exact retry.
    /// Rotation publishes a new revision; terminal revocation requires a new reference.
    pub async fn put_mcp_credential(
        &self,
        scope: &CredentialScope,
        sealed: &SealedCredential,
        expected: i64,
    ) -> Result<McpCredentialRecord> {
        if expected < 0 || expected.checked_add(1) != Some(scope.revision) {
            return Err(Error::Conflict);
        }
        let record = McpCredentialRecord {
            scope: scope.clone(),
            sealed: Some(sealed.clone()),
            revision: scope.revision,
            revoked: false,
        };
        record.validate()?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::native_key_retirement::allow_cipher(&mut tx,&sealed.key_id().cipher_id().map_err(|_|Error::Forbidden)?).await?;
        if sqlx::query("UPDATE projects SET payload=payload WHERE id=$1")
            .bind(&scope.project_id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
            != 1
        {
            return Err(Error::NotFound);
        }
        let row=sqlx::query("SELECT project_id,reference,revision,payload FROM mcp_credentials WHERE project_id=$1 AND reference=$2")
            .bind(&scope.project_id).bind(&scope.reference).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let previous = row.as_ref().map(checked).transpose()?;
        if let Some(old) = &previous {
            if old.revoked {
                return Err(Error::Forbidden);
            }
            if old == &record {
                tx.commit().await.map_err(storage_error)?;
                return Ok(record);
            }
            if old.scope.server_id != scope.server_id || old.scope.endpoint != scope.endpoint {
                return Err(Error::Forbidden);
            }
        }
        if previous.as_ref().map_or(0, |r| r.revision) != expected {
            return Err(Error::Conflict);
        }
        let changed = if expected == 0 {
            sqlx::query("INSERT INTO mcp_credentials(project_id,reference,revision,payload) VALUES($1,$2,$3,$4) ON CONFLICT(project_id,reference) DO NOTHING")
                .bind(&scope.project_id).bind(&scope.reference).bind(record.revision).bind(json_string(&record)?)
                .execute(&mut *tx).await.map_err(storage_error)?.rows_affected()
        } else {
            sqlx::query("UPDATE mcp_credentials SET revision=$1,payload=$2 WHERE project_id=$3 AND reference=$4 AND revision=$5")
                .bind(record.revision).bind(json_string(&record)?).bind(&scope.project_id).bind(&scope.reference).bind(expected)
                .execute(&mut *tx).await.map_err(storage_error)?.rows_affected()
        };
        if changed != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(record)
    }
    pub async fn mcp_credential(
        &self,
        project: &str,
        reference: &str,
    ) -> Result<McpCredentialRecord> {
        let row=sqlx::query("SELECT project_id,reference,revision,payload FROM mcp_credentials WHERE project_id=$1 AND reference=$2")
            .bind(project).bind(reference).fetch_optional(&self.pool).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        checked(&row)
    }
    pub async fn revoke_mcp_credential(
        &self,
        project: &str,
        reference: &str,
        expected: i64,
    ) -> Result<McpCredentialRecord> {
        if expected <= 0 || expected == i64::MAX {
            return Err(Error::Conflict);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let affected = sqlx::query(
            "UPDATE mcp_credentials SET revision=revision WHERE project_id=$1 AND reference=$2",
        )
        .bind(project)
        .bind(reference)
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?
        .rows_affected();
        if affected != 1 {
            return Err(Error::NotFound);
        }
        let row=sqlx::query("SELECT project_id,reference,revision,payload FROM mcp_credentials WHERE project_id=$1 AND reference=$2")
            .bind(project).bind(reference).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let mut record = checked(&row)?;
        if record.revoked && record.revision == expected + 1 {
            tx.commit().await.map_err(storage_error)?;
            return Ok(record);
        }
        if record.revoked || record.revision != expected {
            return Err(Error::Conflict);
        }
        record.revoked = true;
        record.revision += 1;
        record.sealed = None;
        record.validate()?;
        sqlx::query("UPDATE mcp_credentials SET revision=$1,payload=$2 WHERE project_id=$3 AND reference=$4")
            .bind(record.revision).bind(json_string(&record)?).bind(project).bind(reference)
            .execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(record)
    }
    pub async fn mcp_server_credential(
        &self,
        server: &McpServerRecord,
    ) -> Result<Option<McpCredentialRecord>> {
        let Some(scope) = scope_for(server) else {
            return Ok(None);
        };
        let record = self
            .mcp_credential(&scope.project_id, &scope.reference)
            .await?;
        record.sealed_for(&scope)?;
        Ok(Some(record))
    }
}
/// Same row fence as revocation/rotation, held until tool admission commits.
pub(super) async fn check_server(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    server: &McpServerRecord,
) -> Result<()> {
    let Some(scope) = scope_for(server) else {
        return Ok(());
    };
    if sqlx::query("UPDATE mcp_credentials SET revision=revision WHERE project_id=$1 AND reference=$2 AND revision=$3")
        .bind(&scope.project_id).bind(&scope.reference).bind(scope.revision)
        .execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Forbidden);}
    let row=sqlx::query("SELECT project_id,reference,revision,payload FROM mcp_credentials WHERE project_id=$1 AND reference=$2")
        .bind(&scope.project_id).bind(&scope.reference).fetch_one(&mut **tx).await.map_err(storage_error)?;
    checked(&row)?.sealed_for(&scope)?;
    Ok(())
}
