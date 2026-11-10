//! Inert import bytes only, on the caller's identity-locked transaction.
use super::*;
use serde::{Deserialize, Serialize};

pub const IMPORT_STORAGE_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS identity_storage_charges (id TEXT PRIMARY KEY, organization_id TEXT NOT NULL REFERENCES identity_organizations(id), identity_id TEXT NOT NULL REFERENCES identity_subjects(id), resource_id TEXT NOT NULL REFERENCES identity_resources(id), intent_key TEXT NOT NULL, bytes BIGINT NOT NULL CHECK(bytes >= 0), provenance_sha256 TEXT NOT NULL, request_sha256 TEXT NOT NULL, created_at BIGINT NOT NULL, receipt TEXT NOT NULL, UNIQUE(organization_id,identity_id,intent_key))";
pub const IMPORT_STORAGE_RELEASE_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS identity_storage_releases (charge_id TEXT PRIMARY KEY, release_key TEXT NOT NULL UNIQUE, bytes BIGINT NOT NULL CHECK(bytes>0), provenance_sha256 TEXT NOT NULL, original_receipt TEXT NOT NULL, created_at BIGINT NOT NULL)";
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StorageImportRelease {
    format: String,
    original_charge: StorageImportCharge,
    released_bytes: i64,
    release_key: String,
    proof_sha256: String,
    request_sha256: String,
    created_at: i64,
}
impl StorageImportRelease {
    fn validate(&self, charge: &StorageImportCharge) -> Result<()> {
        if self.format != "original-undispatched-storage-release/1"
            || self.original_charge != *charge
            || self.released_bytes <= 0
            || self.released_bytes > charge.bytes
            || !valid_key(&self.release_key)
            || self.proof_sha256.len() != 64
            || !self
                .proof_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.request_sha256
                != digest_request(&(
                    "original-undispatched-storage-release/v1",
                    charge,
                    self.released_bytes,
                    &self.release_key,
                    &self.proof_sha256,
                ))?
        {
            return Err(IdentityError::Corrupt);
        }
        Ok(())
    }
}

impl LockedIdentityQuota<'_, '_, '_> {
    /// Trusted content-storage settlement only. The caller must hold the exact
    /// original never-dispatched operation CAS in this same outer transaction.
    /// This releases no execution/model/concurrency reservation or authority.
    pub async fn release_storage_in_transaction(
        &mut self,
        original: &StorageImportCharge,
        bytes: i64,
        release_key: &str,
        proof_sha256: &str,
    ) -> Result<()> {
        self.usable()?;
        self.poisoned = true;
        let mut operation = self.tx.begin().await?;
        let result = self
            .store
            .release_import_storage_locked(
                &mut operation,
                original,
                bytes,
                release_key,
                proof_sha256,
            )
            .await;
        finish_quota_operation(operation, result, &mut self.poisoned).await
    }
}
impl IdentityStore {
    async fn release_import_storage_locked(
        &self,
        tx: &mut Transaction<'_, Any>,
        original: &StorageImportCharge,
        bytes: i64,
        release_key: &str,
        proof_sha256: &str,
    ) -> Result<()> {
        if bytes <= 0
            || bytes > original.bytes
            || !valid_key(release_key)
            || proof_sha256.len() != 64
            || !proof_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(IdentityError::Validation);
        }
        let row=sqlx::query("SELECT receipt FROM identity_storage_charges WHERE id=$1 AND organization_id=$2 AND identity_id=$3 AND resource_id=$4 AND intent_key=$5 AND bytes=$6 AND provenance_sha256=$7 AND request_sha256=$8").bind(original.id.to_string()).bind(original.organization_id.to_string()).bind(original.identity_id.to_string()).bind(original.resource_id.to_string()).bind(&original.key).bind(original.bytes).bind(&original.provenance_sha256).bind(&original.request_sha256).fetch_optional(&mut **tx).await?.ok_or(IdentityError::NotFound)?;
        if row.try_get::<String, _>("receipt")? != encoded(original)? {
            return Err(IdentityError::Corrupt);
        }
        let request_sha256 = digest_request(&(
            "original-undispatched-storage-release/v1",
            original,
            bytes,
            release_key,
            proof_sha256,
        ))?;
        if let Some(row)=sqlx::query("SELECT release_key,bytes,provenance_sha256,original_receipt,created_at FROM identity_storage_releases WHERE charge_id=$1").bind(original.id.to_string()).fetch_optional(&mut **tx).await? {
            let receipt:StorageImportRelease=decoded(&row.try_get::<String,_>("original_receipt")?)?;
            receipt.validate(original)?;
            if row.try_get::<String,_>("release_key")?!=release_key||row.try_get::<i64,_>("bytes")?!=bytes||row.try_get::<String,_>("provenance_sha256")?!=proof_sha256||row.try_get::<i64,_>("created_at")?!=receipt.created_at||receipt.request_sha256!=request_sha256{ return Err(IdentityError::Conflict); }
            return Ok(());
        }
        for scope in [
            QuotaScope::Platform,
            QuotaScope::Organization(original.organization_id),
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
                .checked_sub(bytes)
                .filter(|n| *n >= 0)
                .ok_or(IdentityError::Corrupt)?;
            quota.revision = quota
                .revision
                .checked_add(1)
                .ok_or(IdentityError::Corrupt)?;
            self.save_quota(tx, &quota).await?;
        }
        let created_at = self.sample(tx).await?;
        let receipt = StorageImportRelease {
            format: "original-undispatched-storage-release/1".into(),
            original_charge: original.clone(),
            released_bytes: bytes,
            release_key: release_key.into(),
            proof_sha256: proof_sha256.into(),
            request_sha256,
            created_at,
        };
        sqlx::query("INSERT INTO identity_storage_releases(charge_id,release_key,bytes,provenance_sha256,original_receipt,created_at) VALUES($1,$2,$3,$4,$5,$6)").bind(original.id.to_string()).bind(release_key).bind(bytes).bind(proof_sha256).bind(encoded(&receipt)?).bind(created_at).execute(&mut **tx).await?;
        Ok(())
    }
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
            let rows = sqlx::query("SELECT c.id,c.organization_id,c.identity_id,c.resource_id,c.intent_key,c.bytes,c.provenance_sha256,c.request_sha256,c.created_at,c.receipt,r.bytes AS released,r.release_key,r.provenance_sha256 AS release_proof,r.original_receipt AS release_receipt,r.created_at AS released_at FROM identity_storage_charges c LEFT JOIN identity_storage_releases r ON r.charge_id=c.id WHERE ($1='' OR c.organization_id=$1) AND c.id>$2 ORDER BY c.id LIMIT 256")
                .bind(&org).bind(&after).fetch_all(&mut **tx).await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let bytes: i64 = row.try_get("bytes")?;
                let charge: StorageImportCharge = decoded(&row.try_get::<String, _>("receipt")?)?;
                if charge.id.to_string() != row.try_get::<String, _>("id")?
                    || charge.organization_id.to_string()
                        != row.try_get::<String, _>("organization_id")?
                    || charge.identity_id.to_string() != row.try_get::<String, _>("identity_id")?
                    || charge.resource_id.to_string() != row.try_get::<String, _>("resource_id")?
                    || charge.key != row.try_get::<String, _>("intent_key")?
                    || charge.bytes != bytes
                    || charge.provenance_sha256 != row.try_get::<String, _>("provenance_sha256")?
                    || charge.request_sha256 != row.try_get::<String, _>("request_sha256")?
                    || charge.created_at != row.try_get::<i64, _>("created_at")?
                    || charge.request_sha256
                        != digest_request(&(
                            "inert_history_storage_charge/v1",
                            charge.organization_id,
                            charge.identity_id,
                            charge.resource_id,
                            &charge.key,
                            bytes,
                            &charge.provenance_sha256,
                        ))?
                {
                    return Err(IdentityError::Corrupt);
                }
                let released = if let Some(raw) =
                    row.try_get::<Option<String>, _>("release_receipt")?
                {
                    let release: StorageImportRelease = decoded(&raw)?;
                    release.validate(&charge)?;
                    if row.try_get::<Option<i64>, _>("released")? != Some(release.released_bytes)
                        || row.try_get::<Option<String>, _>("release_key")?.as_deref()
                            != Some(release.release_key.as_str())
                        || row
                            .try_get::<Option<String>, _>("release_proof")?
                            .as_deref()
                            != Some(release.proof_sha256.as_str())
                        || row.try_get::<Option<i64>, _>("released_at")? != Some(release.created_at)
                    {
                        return Err(IdentityError::Corrupt);
                    }
                    release.released_bytes
                } else {
                    0
                };
                if bytes < 0 || released < 0 || released > bytes {
                    return Err(IdentityError::Corrupt);
                }
                consumed = consumed
                    .checked_add(bytes - released)
                    .ok_or(IdentityError::Corrupt)?;
                after = row.try_get("id")?;
                identifier(after.clone())?;
            }
        }
        Ok(consumed)
    }
}
