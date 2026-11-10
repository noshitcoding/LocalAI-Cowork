//! Original V1 administrative wire over current V2 identities and real membership authority.
use super::*;
use crate::android_devices::{device_iso, device_writer};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction};
use team_service::identity_error;

fn invalid() -> Error {
    Error::Validation("android_administration_invalid".into())
}
fn parse_id(value: &str) -> Result<Uuid> {
    Uuid::parse_str(value)
        .ok()
        .filter(|v| !v.is_nil() && v.to_string() == value)
        .ok_or_else(invalid)
}
async fn audit(
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    team: Uuid,
    target: &str,
    action: &str,
    current: i64,
) -> Result<()> {
    let sequence: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(sequence),0)+1 FROM identity_audit")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    sqlx::query("INSERT INTO identity_audit(id,sequence,organization_id,actor_id,action,target_id,observed_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(id()).bind(sequence).bind(team.to_string()).bind(actor.identity_id().to_string()).bind(action).bind(target).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
impl Store {
    pub async fn android_admin_team_members(
        &self,
        actor: &Principal,
        team: Uuid,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .organization_in_transaction(&mut tx, actor, team)
            .await
            .map_err(identity_error)?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(identity_error)?;
        let rows=sqlx::query("SELECT m.id,m.identity_id,m.role,COALESCE((SELECT MIN(v.email) FROM android_auth_invitations v WHERE v.accepted_user_id=s.id AND v.accepted_at>0),s.username) AS username,s.display_name FROM identity_memberships m JOIN identity_subjects s ON s.id=m.identity_id WHERE m.organization_id=$1 AND m.active=1 AND s.active=1 ORDER BY m.role,m.identity_id LIMIT 10001")
            .bind(team.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 10000 {
            return Err(Error::BudgetExceeded);
        }
        let mut values = Vec::new();
        for r in rows {
            let role: String = r.try_get("role").map_err(storage_error)?;
            // The original V1 enum has no viewer. Never present that role as a
            // member/admin with capabilities it does not possess.
            if !matches!(role.as_str(), "owner" | "admin" | "member") {
                return Err(Error::Validation(
                    "android_v1_unrepresentable_membership_role".into(),
                ));
            }
            let member: String = r.try_get("id").map_err(storage_error)?;
            let created:Option<i64>=sqlx::query_scalar("SELECT COALESCE((SELECT MIN(observed_at) FROM identity_audit WHERE target_id=$1 AND action='member_added'),(SELECT MIN(observed_at) FROM identity_audit WHERE target_id=$2 AND action='organization_created'))")
                .bind(&member).bind(team.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            values.push(json!({"schema_version":2,"team_id":team,"user_id":parse_id(&r.try_get::<String,_>("identity_id").map_err(storage_error)?)?,"role":role,"email":r.try_get::<String,_>("username").map_err(storage_error)?,"display_name":r.try_get::<String,_>("display_name").map_err(storage_error)?,"created_at":device_iso(created.ok_or(Error::Forbidden)?)?}));
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_admin_team_member(
        &self,
        actor: &Principal,
        team: Uuid,
        user: Uuid,
        role: Option<&str>,
        operation: &str,
    ) -> Result<()> {
        if user.is_nil()
            || operation.is_empty()
            || operation.len() > 200
            || operation.chars().any(char::is_control)
            || role.is_some_and(|v| !matches!(v, "admin" | "member"))
        {
            return Err(invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        self.identity()
            .organization_in_transaction(&mut tx, actor, team)
            .await
            .map_err(identity_error)?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Share)
            .await
            .map_err(identity_error)?;
        let caller:Option<String>=sqlx::query_scalar("SELECT role FROM identity_memberships WHERE organization_id=$1 AND identity_id=$2 AND active=1")
            .bind(team.to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let caller = caller.ok_or(Error::Forbidden)?;
        if !matches!(caller.as_str(), "owner" | "admin") {
            return Err(Error::Forbidden);
        }
        let request = canonical_hash(
            &json!({"purpose":"android_team_member/v1","team":team,"user":user,"role":role}),
        );
        if let Some(old)=sqlx::query_scalar::<_,String>("SELECT request_sha256 FROM android_admin_operations WHERE actor_id=$1 AND scope_id=$2 AND operation_id=$3")
            .bind(actor.identity_id().to_string()).bind(team.to_string()).bind(operation).fetch_optional(&mut *tx).await.map_err(storage_error)? {
            if old!=request {return Err(Error::Conflict);}
            tx.commit().await.map_err(storage_error)?;return Ok(());
        }
        let retained: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM android_admin_operations WHERE actor_id=$1")
                .bind(actor.identity_id().to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if retained >= 4096 {
            return Err(Error::BudgetExceeded);
        }
        let subject: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM identity_subjects WHERE id=$1 AND active=1")
                .bind(user.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if subject != 1 {
            return Err(Error::NotFound);
        }
        let target=sqlx::query("SELECT id,role,revision FROM identity_memberships WHERE organization_id=$1 AND identity_id=$2 AND active=1")
            .bind(team.to_string()).bind(user.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if target
            .as_ref()
            .is_some_and(|r| r.try_get::<String, _>("role").ok().as_deref() == Some("owner"))
        {
            return Err(Error::Forbidden);
        }
        if caller == "admin"
            && (role == Some("admin")
                || target.as_ref().is_some_and(|r| {
                    r.try_get::<String, _>("role").ok().as_deref() == Some("admin")
                }))
        {
            return Err(Error::Forbidden);
        }
        let mut changed = false;
        if let Some(target) = target {
            let old_role: String = target.try_get("role").map_err(storage_error)?;
            let target_id: String = target.try_get("id").map_err(storage_error)?;
            if role != Some(old_role.as_str()) {
                if sqlx::query("UPDATE identity_memberships SET role=$1,active=$2,revision=revision+1 WHERE id=$3 AND active=1 AND revision=$4")
                    .bind(role.unwrap_or(old_role.as_str())).bind(i64::from(role.is_some())).bind(&target_id).bind(target.try_get::<i64,_>("revision").map_err(storage_error)?)
                    .execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
                audit(
                    &mut tx,
                    actor,
                    team,
                    &target_id,
                    if role.is_some() {
                        "member_role_changed"
                    } else {
                        "member_revoked"
                    },
                    current,
                )
                .await?;
                changed = true;
            }
        } else if let Some(role) = role {
            let member = id();
            sqlx::query("INSERT INTO identity_memberships(id,organization_id,identity_id,role,active,revision) VALUES($1,$2,$3,$4,1,1)")
                .bind(&member).bind(team.to_string()).bind(user.to_string()).bind(role).execute(&mut *tx).await.map_err(storage_error)?;
            audit(&mut tx, actor, team, &member, "member_added", current).await?;
            changed = true;
        } else {
            return Err(Error::NotFound);
        }
        if changed {
            sqlx::query("UPDATE identity_organizations SET authorization_epoch=authorization_epoch+1,revision=revision+1 WHERE id=$1").bind(team.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        }
        sqlx::query("INSERT INTO android_admin_operations(actor_id,scope_id,operation_id,request_sha256,receipt,created_at) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(actor.identity_id().to_string()).bind(team.to_string()).bind(operation).bind(request).bind("{\"status\":204}").bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
}
