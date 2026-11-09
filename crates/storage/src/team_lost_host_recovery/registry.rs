use super::*;
pub(super) async fn lock(store: &Store, tx: &mut Transaction<'_, Any>) -> Result<()> {
    let changed = sqlx::query("UPDATE identity_installation SET revision=revision WHERE id=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    if changed.rows_affected() != 1 {
        return Err(Error::Forbidden);
    }
    let _ = store;
    Ok(())
}
pub(super) fn profile(row: &AnyRow) -> Result<OperatorHostProfile> {
    let original: String = row.try_get("profile").map_err(storage_error)?;
    let value: OperatorHostProfile = parse(&original)?;
    value.validate(&value.binding)?;
    if digest(&value)?
        != row
            .try_get::<String, _>("profile_sha256")
            .map_err(storage_error)?
        || value.public_key_sha256
            != row
                .try_get::<String, _>("public_key_sha256")
                .map_err(storage_error)?
        || value.public_key_der
            != row
                .try_get::<String, _>("public_key_der")
                .map_err(storage_error)?
        || value.id.to_string() != row.try_get::<String, _>("id").map_err(storage_error)?
        || value.team_id.to_string()
            != row.try_get::<String, _>("team_id").map_err(storage_error)?
        || value.vm_id.to_string() != row.try_get::<String, _>("vm_id").map_err(storage_error)?
        || value.binding.host_id.to_string()
            != row.try_get::<String, _>("host_id").map_err(storage_error)?
        || value.binding.engine_id
            != row
                .try_get::<String, _>("engine_id")
                .map_err(storage_error)?
        || value.binding.node_name
            != row
                .try_get::<Option<String>, _>("node_name")
                .map_err(storage_error)?
        || value.binding.node_uid.map(|v| v.to_string())
            != row
                .try_get::<Option<String>, _>("node_uid")
                .map_err(storage_error)?
        || value.execution_epoch.to_string()
            != row
                .try_get::<String, _>("execution_epoch")
                .map_err(storage_error)?
    {
        return Err(Error::Conflict);
    }
    Ok(value)
}
pub(super) fn view(row: &AnyRow) -> Result<TeamHostTrustProfileView> {
    let p = profile(row)?;
    Ok(TeamHostTrustProfileView {
        id: p.id,
        team_id: p.team_id,
        binding: p.binding,
        profile_sha256: row.try_get("profile_sha256").map_err(storage_error)?,
        public_key_sha256: p.public_key_sha256,
        revision: row.try_get("revision").map_err(storage_error)?,
        active: row.try_get::<i64, _>("active").map_err(storage_error)? == 1,
        inspected_at: row.try_get("inspected_at").map_err(storage_error)?,
        activated_at: row.try_get("activated_at").map_err(storage_error)?,
    })
}
impl Store {
    /// Only the private local operator CLI calls this; there is deliberately no HTTP enrollment endpoint.
    /// Inspect does not write, activate is an explicit source/CAS admission, restart never reactivates.
    pub async fn operator_team_host_trust(
        &self,
        p: OperatorHostProfile,
        observed: TeamOriginalHostBinding,
        action: &str,
        expected: i64,
        operation: Uuid,
    ) -> Result<Value> {
        p.validate(&observed)?;
        if !matches!(action, "inspect" | "activate" | "revoke")
            || expected < 0
            || operation.is_nil()
        {
            return Err(invalid());
        }
        let request = serde_json::json!({"profile_id":p.id,"source_sha256":digest(&p)?,"observed":observed,"action":action,"expected_revision":expected,"operation_id":operation});
        let request_sha = digest(&request)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        lock(self, &mut tx).await?;
        let org: Option<String> = sqlx::query_scalar(
            "SELECT id FROM identity_organizations WHERE id=$1 AND state='active'",
        )
        .bind(p.team_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
        if org.is_none() {
            return Err(Error::Forbidden);
        }
        if let Some(old)=sqlx::query("SELECT request_sha256,receipt FROM team_host_trust_operations WHERE profile_id=$1 AND operation_id=$2").bind(p.id.to_string()).bind(operation.to_string()).fetch_optional(&mut*tx).await.map_err(storage_error)?{if old.try_get::<String,_>("request_sha256").map_err(storage_error)?!=request_sha{return Err(Error::Conflict);}let receipt=parse(&old.try_get::<String,_>("receipt").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(receipt);}
        let old = sqlx::query("SELECT * FROM team_host_trust_profiles WHERE id=$1")
            .bind(p.id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        if let Some(row) = &old {
            if digest(&profile(row)?)? != digest(&p)?
                || row.try_get::<i64, _>("revision").map_err(storage_error)? != expected
            {
                return Err(Error::Conflict);
            }
        } else if expected != 0 || action == "revoke" {
            return Err(Error::Conflict);
        }
        if action == "inspect" {
            let result = if let Some(row) = &old {
                serde_json::to_value(view(row)?).map_err(storage_error)?
            } else {
                serde_json::json!({"id":p.id,"team_id":p.team_id,"binding":observed,"profile_sha256":digest(&p)?,"public_key_sha256":p.public_key_sha256,"revision":0,"active":false,"registered":false})
            };
            tx.commit().await.map_err(storage_error)?;
            return Ok(result);
        }
        let clock = db_now(self, &mut tx).await?;
        if action == "activate" {
            let unbound:i64=sqlx::query_scalar("SELECT COUNT(*) FROM team_container_host_admissions WHERE host_id=$1 AND engine_id=$2 AND profile_id IS NULL AND normal_removed_receipt IS NULL AND retired_receipt_sha256 IS NULL").bind(p.binding.host_id.to_string()).bind(&p.binding.engine_id).fetch_one(&mut*tx).await.map_err(storage_error)?;
            if unbound != 0 {
                return Err(Error::UncertainEffect);
            }
            let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM team_lost_host_retirement_operations o JOIN team_container_host_admissions a ON a.nonce=o.nonce JOIN team_host_trust_profiles p ON p.id=a.profile_id WHERE p.vm_id=$1").bind(p.vm_id.to_string()).fetch_one(&mut*tx).await.map_err(storage_error)?;
            if pending != 0 {
                return Err(Error::Conflict);
            }
            let retired:i64=sqlx::query_scalar("SELECT COUNT(*) FROM team_host_trust_profiles WHERE vm_id=$1 AND retired_at IS NOT NULL").bind(p.vm_id.to_string()).fetch_one(&mut*tx).await.map_err(storage_error)?;
            if retired != 0
                || old.as_ref().is_some_and(|r| {
                    r.try_get::<Option<i64>, _>("retired_at")
                        .ok()
                        .flatten()
                        .is_some()
                })
            {
                return Err(Error::Conflict);
            }
            let other:i64=sqlx::query_scalar("SELECT COUNT(*) FROM team_host_trust_profiles WHERE host_id=$1 AND engine_id=$2 AND active=1 AND id<>$3").bind(p.binding.host_id.to_string()).bind(&p.binding.engine_id).bind(p.id.to_string()).fetch_one(&mut*tx).await.map_err(storage_error)?;
            if other != 0 {
                return Err(Error::Conflict);
            }
        }
        if old.is_none() {
            sqlx::query("INSERT INTO team_host_trust_profiles(id,team_id,host_id,engine_id,node_name,node_uid,profile_sha256,profile,public_key_der,public_key_sha256,vm_id,execution_epoch,revision,active,inspected_at,activated_at,creator_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,1,1,$13,$13,NULL)").bind(p.id.to_string()).bind(p.team_id.to_string()).bind(p.binding.host_id.to_string()).bind(&p.binding.engine_id).bind(&p.binding.node_name).bind(p.binding.node_uid.map(|v|v.to_string())).bind(digest(&p)?).bind(json_string(&p)?).bind(&p.public_key_der).bind(&p.public_key_sha256).bind(p.vm_id.to_string()).bind(p.execution_epoch.to_string()).bind(clock).execute(&mut*tx).await.map_err(storage_error)?;
        } else {
            let changed=sqlx::query("UPDATE team_host_trust_profiles SET revision=revision+1,active=$1,inspected_at=$2,activated_at=CASE WHEN $1=1 THEN $2 ELSE activated_at END WHERE id=$3 AND revision=$4 AND retired_at IS NULL").bind(i64::from(action=="activate")).bind(clock).bind(p.id.to_string()).bind(expected).execute(&mut*tx).await.map_err(storage_error)?;
            if changed.rows_affected() != 1 {
                return Err(Error::Conflict);
            }
        }
        let row = sqlx::query("SELECT * FROM team_host_trust_profiles WHERE id=$1")
            .bind(p.id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let receipt = serde_json::json!({"format":"localai-original-host-trust-operator-receipt/1","operation_id":operation,"action":action,"request_sha256":request_sha,"original_source_sha256":digest(&p)?,"observed_binding":observed,"profile":view(&row)?,"created_at":clock,"automatic_activation":false});
        sqlx::query("INSERT INTO team_host_trust_operations(actor_id,operation_id,profile_id,request_sha256,receipt,created_at) VALUES(NULL,$1,$2,$3,$4,$5)").bind(operation.to_string()).bind(p.id.to_string()).bind(&request_sha).bind(json_string(&receipt)?).bind(clock).execute(&mut*tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
