//! Actual current Actor storage budget and original administrative replies. No model/token budget is introduced.
use super::*;
use crate::android_devices::{device_invalid, device_writer};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction};
pub(crate) async fn charge(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    bytes: i64,
    reserved_delta: i64,
) -> Result<()> {
    if bytes < 0 {
        return Err(device_invalid());
    }
    sqlx::query("INSERT INTO android_personal_storage_quotas(actor_id,revision,limit_bytes,consumed_bytes,reserved_bytes) VALUES($1,1,536870912,0,0) ON CONFLICT(actor_id) DO NOTHING").bind(owner.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
    let r = sqlx::query("SELECT * FROM android_personal_storage_quotas WHERE actor_id=$1")
        .bind(owner.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let consumed = r
        .try_get::<i64, _>("consumed_bytes")
        .map_err(storage_error)?
        .checked_add(bytes)
        .ok_or(Error::BudgetExceeded)?;
    let reserved = r
        .try_get::<i64, _>("reserved_bytes")
        .map_err(storage_error)?
        .checked_add(reserved_delta)
        .filter(|v| *v >= 0)
        .ok_or(Error::Conflict)?;
    if consumed
        .checked_add(reserved)
        .is_none_or(|n| n > r.try_get::<i64, _>("limit_bytes").unwrap_or(0))
    {
        return Err(Error::BudgetExceeded);
    }
    let revision: i64 = r.try_get("revision").map_err(storage_error)?;
    if sqlx::query("UPDATE android_personal_storage_quotas SET consumed_bytes=$1,reserved_bytes=$2,revision=revision+1 WHERE actor_id=$3 AND revision=$4").bind(consumed).bind(reserved).bind(owner.to_string()).bind(revision).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
    Ok(())
}
pub(crate) async fn original(
    tx: &mut Transaction<'_, Any>,
    actor: Uuid,
    key: &str,
    purpose: &str,
    sha: &str,
) -> Result<Option<Value>> {
    let Some(r)=sqlx::query("SELECT purpose,request_sha256,receipt FROM android_administration_operations WHERE actor_id=$1 AND operation_key=$2").bind(actor.to_string()).bind(key).fetch_optional(&mut **tx).await.map_err(storage_error)?else{return Ok(None)};
    if r.try_get::<String, _>("purpose").map_err(storage_error)? != purpose
        || r.try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != sha
    {
        return Err(Error::Conflict);
    }
    let v: Value = parse(&r.try_get::<String, _>("receipt").map_err(storage_error)?)?;
    if v["request_sha256"] != sha {
        return Err(Error::Forbidden);
    }
    Ok(Some(v))
}
pub(crate) async fn remember(
    tx: &mut Transaction<'_, Any>,
    actor: Uuid,
    key: &str,
    purpose: &str,
    sha: &str,
    receipt: &Value,
    current: i64,
) -> Result<()> {
    if key.is_empty() || key.len() > 200 || json_string(receipt)?.len() > 512 * 1024 {
        return Err(device_invalid());
    }
    sqlx::query("INSERT INTO android_administration_operations(actor_id,operation_key,purpose,request_sha256,receipt,created_at) VALUES($1,$2,$3,$4,$5,$6)").bind(actor.to_string()).bind(key).bind(purpose).bind(sha).bind(json_string(receipt)?).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
impl Store {
    pub async fn android_administration_original(
        &self,
        actor: &Principal,
        key: &str,
        purpose: &str,
        sha: &str,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        if !matches!(
            purpose,
            "support_create" | "support_revoke" | "pool_profile" | "pool_project" | "executor_sync"
        ) {
            self.identity()
                .require_global_session_in_transaction(&mut tx, actor)
                .await
                .map_err(team_service::identity_error)?;
        }
        let v = original(&mut tx, actor.identity_id(), key, purpose, sha)
            .await?
            .ok_or(Error::NotFound)?;
        if matches!(
            purpose,
            "support_create" | "support_revoke" | "pool_profile" | "pool_project" | "executor_sync"
        ) {
            crate::android_support_grants::receipt_scope(self, &mut tx, actor, &v).await?;
        }
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_personal_storage_quota(
        &self,
        actor: &Principal,
        target: Uuid,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let actual = self
            .identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(team_service::identity_error)?;
        self.identity()
            .require_global_session_in_transaction(&mut tx, actor)
            .await
            .map_err(team_service::identity_error)?;
        if target != actor.identity_id() && !actual.platform_admin {
            return Err(Error::Forbidden);
        }
        let row = sqlx::query("SELECT * FROM android_personal_storage_quotas WHERE actor_id=$1")
            .bind(target.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let v = match row {
            Some(r) => {
                json!({"actor_id":target,"revision":r.try_get::<i64,_>("revision").map_err(storage_error)?,"limit_bytes":r.try_get::<i64,_>("limit_bytes").map_err(storage_error)?,"consumed_bytes":r.try_get::<i64,_>("consumed_bytes").map_err(storage_error)?,"reserved_bytes":r.try_get::<i64,_>("reserved_bytes").map_err(storage_error)?})
            }
            None => {
                json!({"actor_id":target,"revision":0,"limit_bytes":536870912u64,"consumed_bytes":0,"reserved_bytes":0})
            }
        };
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_personal_storage_quota_set(
        &self,
        actor: &Principal,
        target: Uuid,
        expected: i64,
        limit: i64,
        request: Uuid,
    ) -> Result<Value> {
        if expected < 0 || limit < 0 || limit > 16 * 1024 * 1024 * 1024 || request.is_nil() {
            return Err(device_invalid());
        }
        let sha = canonical_hash(
            &json!({"actor_id":target,"expected_revision":expected,"limit_bytes":limit,"request_id":request}),
        );
        let key = format!("storage-quota:{request}");
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        self.identity()
            .require_global_session_in_transaction(&mut tx, actor)
            .await
            .map_err(team_service::identity_error)?;
        let actual = self
            .identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(team_service::identity_error)?;
        if !actual.platform_admin {
            return Err(Error::Forbidden);
        }
        if let Some(v) = original(&mut tx, actor.identity_id(), &key, "storage_quota", &sha).await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Write)
            .await
            .map_err(team_service::identity_error)?;
        let r = sqlx::query("SELECT * FROM android_personal_storage_quotas WHERE actor_id=$1")
            .bind(target.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let revision = r
            .as_ref()
            .map(|r| r.try_get::<i64, _>("revision").map_err(storage_error))
            .transpose()?
            .unwrap_or(0);
        if revision != expected {
            return Err(Error::Conflict);
        }
        let used = r
            .as_ref()
            .map(|r| {
                Ok::<_, Error>(
                    r.try_get::<i64, _>("consumed_bytes")
                        .map_err(storage_error)?
                        .checked_add(
                            r.try_get::<i64, _>("reserved_bytes")
                                .map_err(storage_error)?,
                        )
                        .ok_or(Error::BudgetExceeded)?,
                )
            })
            .transpose()?
            .unwrap_or(0);
        if used > limit {
            return Err(Error::BudgetExceeded);
        }
        let active: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM identity_subjects WHERE id=$1 AND active=1")
                .bind(target.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if active != 1 {
            return Err(Error::NotFound);
        }
        sqlx::query("INSERT INTO android_personal_storage_quotas(actor_id,revision,limit_bytes,consumed_bytes,reserved_bytes) VALUES($1,1,$2,0,0) ON CONFLICT(actor_id) DO UPDATE SET revision=revision+1,limit_bytes=$2 WHERE android_personal_storage_quotas.revision=$3").bind(target.to_string()).bind(limit).bind(expected).execute(&mut *tx).await.map_err(storage_error)?;
        self.identity()
            .android_record_storage_quota_in_transaction(&mut tx, actor, target)
            .await
            .map_err(team_service::identity_error)?;
        let receipt = json!({"schema_version":2,"actor_id":target,"revision":expected+1,"limit_bytes":limit,"request_id":request,"request_sha256":sha});
        remember(
            &mut tx,
            actor.identity_id(),
            &key,
            "storage_quota",
            &sha,
            &receipt,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}

impl Store {
    pub async fn android_operational_status(
        &self,
        actor: &Principal,
        project: Option<Uuid>,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        if let Some(p) = project {
            self.android_task_project_permit(&mut tx, actor, p, ContentAction::Read)
                .await?;
        } else {
            self.identity()
                .require_global_session_in_transaction(&mut tx, actor)
                .await
                .map_err(team_service::identity_error)?;
        }
        let owner = actor.identity_id().to_string();
        let target = project.map(|p| p.to_string());
        let devices: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_devices WHERE identity_id=$1 AND revoked=0",
        )
        .bind(&owner)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let cache: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_mobile_cache_imports WHERE owner_id=$1",
        )
        .bind(&owner)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let runs:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_server_runs WHERE owner_id=$1 AND ($2 IS NULL OR project_id=$2)").bind(&owner).bind(&target).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let unknown:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_loop_steps s JOIN android_private_server_runs r ON r.id=s.run_id WHERE r.owner_id=$1 AND ($2 IS NULL OR r.project_id=$2) AND s.state IN('admitted','uncertain')").bind(&owner).bind(&target).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if let Some(p) = project {
            self.android_task_project_permit(&mut tx, actor, p, ContentAction::Read)
                .await?;
        }
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"schema_version":2,"actor_id":actor.identity_id(),"project_id":project,"paired_active_devices":devices,"cache_imports":cache,"own_private_runs":runs,"own_unresolved_private_steps":unknown,"sync_schema_version":2,"cache_archive_max_bytes":crate::android_mobile_cache::CACHE_ARCHIVE_MAX,"cache_part_max_bytes":crate::android_mobile_cache::CACHE_PART,"contains_secrets":false,"contains_other_actor_runs":false}),
        )
    }
}
