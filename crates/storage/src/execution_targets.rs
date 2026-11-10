//! Explicit installation-operator registry. Tenant sessions and renderer bodies
//! never call these mutations. Every revision is retained, including revocation.
use super::*;
use cowork_domain::execution_targets::*;
use sqlx::{Any, Transaction, any::AnyRow};

fn state(value: ExecutionTargetState) -> &'static str {
    match value {
        ExecutionTargetState::Registered => "registered",
        ExecutionTargetState::Active => "active",
        ExecutionTargetState::Revoked => "revoked",
    }
}
fn record(row: AnyRow) -> Result<ExecutionTarget> {
    let raw: String = row.try_get("payload").map_err(storage_error)?;
    let value: ExecutionTarget = parse(&raw)?;
    if row
        .try_get::<Option<String>, _>("revision_payload")
        .map_err(storage_error)?
        .as_deref()
        != Some(raw.as_str())
    {
        return Err(Error::Conflict);
    }
    validate_target(&value)?;
    if value.target_id
        != row
            .try_get::<String, _>("target_id")
            .map_err(storage_error)?
        || value.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || value.identity_sha256
            != row
                .try_get::<String, _>("identity_sha256")
                .map_err(storage_error)?
        || state(value.state) != row.try_get::<String, _>("state").map_err(storage_error)?
    {
        return Err(Error::Conflict);
    }
    Ok(value)
}
async fn writer(tx: &mut Transaction<'_, Any>) -> Result<()> {
    let locked = sqlx::query("UPDATE execution_target_registry_guard SET id=id WHERE id=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    if locked.rows_affected() != 1 {
        return Err(Error::Conflict);
    }
    Ok(())
}
async fn existing(tx: &mut Transaction<'_, Any>, id: &str) -> Result<Option<ExecutionTarget>> {
    sqlx::query("SELECT t.*,h.payload AS revision_payload FROM execution_targets t LEFT JOIN execution_target_revisions h ON h.target_id=t.target_id AND h.revision=t.revision WHERE t.target_id=$1").bind(id)
        .fetch_optional(&mut **tx).await.map_err(storage_error)?.map(record).transpose()
}
async fn save(tx: &mut Transaction<'_, Any>, value: &ExecutionTarget, initial: bool) -> Result<()> {
    validate_target(value)?;
    let payload = json_string(value)?;
    if initial {
        sqlx::query("INSERT INTO execution_targets(target_id,revision,identity_sha256,state,payload) VALUES($1,$2,$3,$4,$5)")
            .bind(&value.target_id).bind(value.revision).bind(&value.identity_sha256).bind(state(value.state)).bind(&payload)
            .execute(&mut **tx).await.map_err(storage_error)?;
    } else {
        let updated=sqlx::query("UPDATE execution_targets SET revision=$1,identity_sha256=$2,state=$3,payload=$4 WHERE target_id=$5 AND revision=$6")
            .bind(value.revision).bind(&value.identity_sha256).bind(state(value.state)).bind(&payload)
            .bind(&value.target_id).bind(value.revision-1).execute(&mut **tx).await.map_err(storage_error)?;
        if updated.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
    }
    sqlx::query(
        "INSERT INTO execution_target_revisions(target_id,revision,payload) VALUES($1,$2,$3)",
    )
    .bind(&value.target_id)
    .bind(value.revision)
    .bind(payload)
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?;
    Ok(())
}

impl Store {
    /// Local trusted operator operation. Reading a private configuration is not
    /// activation; new identities and new generations always start registered.
    pub async fn register_execution_target(
        &self,
        identity: &ExecutionRunnerIdentity,
        expected_revision: i64,
    ) -> Result<ExecutionTarget> {
        let id = execution_target_id(identity)?;
        if expected_revision < 0 {
            return Err(Error::Conflict);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        writer(&mut tx).await?;
        let old = existing(&mut tx, &id).await?;
        if old.as_ref().map(|value| value.revision).unwrap_or(0) != expected_revision {
            return Err(Error::Conflict);
        }
        let initial = old.is_none();
        if let Some(value) = &old {
            if value.identity == *identity {
                tx.commit().await.map_err(storage_error)?;
                return Ok(value.clone());
            }
            if identity.generation <= value.identity.generation {
                return Err(Error::Forbidden);
            }
        } else {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_targets")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
            if count >= 1024 {
                return Err(Error::BudgetExceeded);
            }
        }
        let at = now().max(old.as_ref().map(|value| value.updated_at).unwrap_or(1));
        let value = ExecutionTarget {
            format_version: 1,
            target_id: id,
            revision: expected_revision
                .checked_add(1)
                .filter(|revision| *revision <= 10000)
                .ok_or(Error::BudgetExceeded)?,
            state: ExecutionTargetState::Registered,
            identity: identity.clone(),
            identity_sha256: runner_identity_hash(identity)?,
            created_at: old.as_ref().map(|value| value.created_at).unwrap_or(at),
            updated_at: at,
        };
        save(&mut tx, &value, initial).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }

    /// An operator changes only the exact identity/revision it inspected.
    pub async fn set_execution_target_active(
        &self,
        identity: &ExecutionRunnerIdentity,
        expected_revision: i64,
        active: bool,
    ) -> Result<ExecutionTarget> {
        let id = execution_target_id(identity)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        writer(&mut tx).await?;
        let mut value = existing(&mut tx, &id).await?.ok_or(Error::NotFound)?;
        if value.revision != expected_revision || value.identity != *identity {
            return Err(Error::Conflict);
        }
        let next = if active {
            ExecutionTargetState::Active
        } else {
            ExecutionTargetState::Revoked
        };
        if value.state == next {
            tx.commit().await.map_err(storage_error)?;
            return Ok(value);
        }
        value.revision = value
            .revision
            .checked_add(1)
            .filter(|revision| *revision <= 10000)
            .ok_or(Error::BudgetExceeded)?;
        value.updated_at = now().max(value.updated_at);
        value.state = next;
        save(&mut tx, &value, false).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }

    pub async fn execution_target(
        &self,
        identity: &ExecutionRunnerIdentity,
    ) -> Result<Option<ExecutionTarget>> {
        let id = execution_target_id(identity)?;
        sqlx::query("SELECT t.*,h.payload AS revision_payload FROM execution_targets t LEFT JOIN execution_target_revisions h ON h.target_id=t.target_id AND h.revision=t.revision WHERE t.target_id=$1").bind(id)
            .fetch_optional(&self.pool).await.map_err(storage_error)?.map(record).transpose()
    }

    pub async fn execution_target_reference(
        &self,
        identity: &ExecutionRunnerIdentity,
    ) -> Result<ExecutionTargetRef> {
        let value = self
            .execution_target(identity)
            .await?
            .ok_or(Error::Forbidden)?;
        if value.identity != *identity {
            return Err(Error::Forbidden);
        }
        target_reference(&value)
    }

    pub async fn validate_execution_target(
        &self,
        reference: &ExecutionTargetRef,
        identity: &ExecutionRunnerIdentity,
    ) -> Result<()> {
        if self.execution_target_reference(identity).await? != *reference {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}

pub(super) async fn check_reference_in_transaction(
    tx: &mut Transaction<'_, Any>,
    reference: &ExecutionTargetRef,
    lock: bool,
) -> Result<ExecutionTarget> {
    if lock {
        writer(tx).await?;
    }
    let value = existing(tx, &reference.target_id)
        .await?
        .ok_or(Error::ExecutionPlanChanged)?;
    let current = target_reference(&value).map_err(|_| Error::ExecutionPlanChanged)?;
    if current != *reference {
        return Err(Error::ExecutionPlanChanged);
    }
    Ok(value)
}
