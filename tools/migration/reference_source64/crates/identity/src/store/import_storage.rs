//! Inert import bytes only, on the caller's identity-locked transaction.
use super::*;
use serde::{Deserialize, Serialize};

pub const IMPORT_STORAGE_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS identity_storage_charges (id TEXT PRIMARY KEY, organization_id TEXT NOT NULL REFERENCES identity_organizations(id), identity_id TEXT NOT NULL REFERENCES identity_subjects(id), resource_id TEXT NOT NULL REFERENCES identity_resources(id), intent_key TEXT NOT NULL, bytes BIGINT NOT NULL CHECK(bytes >= 0), provenance_sha256 TEXT NOT NULL, request_sha256 TEXT NOT NULL, created_at BIGINT NOT NULL, receipt TEXT NOT NULL, UNIQUE(organization_id,identity_id,intent_key))";
pub const MAX_IMPORT_STORAGE_BYTES: i64 = 512 * 1024 * 1024;

/// A content receipt, with no model, execution, concurrency or settlement permit.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StorageImportCharge {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub identity_id: Uuid,
    pub resource_id: Uuid,
    pub key: String,
    pub bytes: i64,
    pub provenance_sha256: String,
    pub request_sha256: String,
    pub created_at: i64,
}

impl LockedIdentityQuota<'_, '_, '_> {
    pub async fn charge_storage_in_transaction(
        &mut self,
        actor: &Principal,
        permit: &ContentPermit,
        key: &str,
        bytes: i64,
        provenance_sha256: &str,
    ) -> Result<StorageImportCharge> {
        self.usable()?;
        self.poisoned = true;
        let mut operation = self.tx.begin().await?;
        let result = self
            .store
            .charge_import_storage_locked(
                &mut operation,
                actor,
                permit,
                key,
                bytes,
                provenance_sha256,
            )
            .await;
        finish_quota_operation(operation, result, &mut self.poisoned).await
    }
}

impl IdentityStore {
    async fn charge_import_storage_locked(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        permit: &ContentPermit,
        key: &str,
        bytes: i64,
        provenance_sha256: &str,
    ) -> Result<StorageImportCharge> {
        if !valid_key(key)
            || !(0..=MAX_IMPORT_STORAGE_BYTES).contains(&bytes)
            || permit.0.action != ContentAction::Write
            || provenance_sha256.len() != 64
            || !provenance_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(IdentityError::Validation);
        }
        if actor.identity_id != permit.0.identity_id || actor.session_id != permit.0.session_id {
            return Err(IdentityError::Forbidden);
        }
        self.validate_content_locked(tx, permit).await?;
        let request_sha256 = digest_request(&(
            "inert_history_storage_charge/v1",
            permit.0.organization_id,
            actor.identity_id,
            permit.0.resource_id,
            key,
            bytes,
            provenance_sha256,
        ))?;
        if let Some(row) = sqlx::query("SELECT request_sha256,receipt,bytes,provenance_sha256,resource_id FROM identity_storage_charges WHERE organization_id=$1 AND identity_id=$2 AND intent_key=$3")
            .bind(permit.0.organization_id.to_string()).bind(actor.identity_id.to_string()).bind(key)
            .fetch_optional(&mut **tx).await? {
            if row.try_get::<String,_>("request_sha256")? != request_sha256
                || row.try_get::<i64,_>("bytes")? != bytes
                || row.try_get::<String,_>("provenance_sha256")? != provenance_sha256
                || row.try_get::<String,_>("resource_id")? != permit.0.resource_id.to_string()
            { return Err(IdentityError::Conflict); }
            let charge: StorageImportCharge = decoded(&row.try_get::<String,_>("receipt")?)?;
            if charge.organization_id != permit.0.organization_id || charge.identity_id != actor.identity_id
                || charge.resource_id != permit.0.resource_id || charge.key != key || charge.bytes != bytes
                || charge.request_sha256 != request_sha256 || charge.provenance_sha256 != provenance_sha256
            { return Err(IdentityError::Corrupt); }
            return Ok(charge);
        }
        // Missing quotas do not invent model/token/concurrency limits. The bounded
        // charge persists; first later quota configuration seeds consumed bytes
        // through storage_import_consumed_in_transaction below.
        for scope in [
            QuotaScope::Platform,
            QuotaScope::Organization(permit.0.organization_id),
        ] {
            if sqlx::query("SELECT scope FROM identity_quotas WHERE scope=$1")
                .bind(scope.key())
                .fetch_optional(&mut **tx)
                .await?
                .is_none()
            {
                continue;
            }
            let mut quota = self.quota(tx, scope).await?;
            quota.consumed.storage_bytes = quota
                .consumed
                .storage_bytes
                .checked_add(bytes)
                .ok_or(IdentityError::QuotaExceeded)?;
            let total = quota
                .consumed
                .storage_bytes
                .checked_add(quota.held.storage_bytes)
                .ok_or(IdentityError::QuotaExceeded)?;
            if total > quota.limit.storage_bytes {
                return Err(IdentityError::QuotaExceeded);
            }
            quota.revision = quota
                .revision
                .checked_add(1)
                .ok_or(IdentityError::Corrupt)?;
            self.save_quota(tx, &quota).await?;
        }
        let charge = StorageImportCharge {
            id: Uuid::new_v4(),
            organization_id: permit.0.organization_id,
            identity_id: actor.identity_id,
            resource_id: permit.0.resource_id,
            key: key.to_owned(),
            bytes,
            provenance_sha256: provenance_sha256.to_owned(),
            request_sha256,
            created_at: self.sample(tx).await?,
        };
        sqlx::query("INSERT INTO identity_storage_charges(id,organization_id,identity_id,resource_id,intent_key,bytes,provenance_sha256,request_sha256,created_at,receipt) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(charge.id.to_string()).bind(charge.organization_id.to_string()).bind(charge.identity_id.to_string())
            .bind(charge.resource_id.to_string()).bind(&charge.key).bind(charge.bytes).bind(&charge.provenance_sha256)
            .bind(&charge.request_sha256).bind(charge.created_at).bind(encoded(&charge)?)
            .execute(&mut **tx).await?;
        Ok(charge)
    }

    /// First configuration only: bounded-memory keyset traversal, checked sum.
    /// Root configure_quota invokes this while holding the installation lock.
    pub(super) async fn storage_import_consumed_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        scope: QuotaScope,
    ) -> Result<i64> {
        let org = match scope {
            QuotaScope::Platform => String::new(),
            QuotaScope::Organization(id) => id.to_string(),
        };
        let mut after = String::new();
        let mut consumed = 0i64;
        loop {
            let rows = sqlx::query("SELECT id,bytes FROM identity_storage_charges WHERE ($1='' OR organization_id=$1) AND id>$2 ORDER BY id LIMIT 256")
                .bind(&org).bind(&after).fetch_all(&mut **tx).await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let bytes: i64 = row.try_get("bytes")?;
                if bytes < 0 {
                    return Err(IdentityError::Corrupt);
                }
                consumed = consumed.checked_add(bytes).ok_or(IdentityError::Corrupt)?;
                after = row.try_get("id")?;
                identifier(after.clone())?;
            }
        }
        Ok(consumed)
    }
}
