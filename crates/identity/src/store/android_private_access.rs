//! Application-owned private project audit; no synthetic organization or membership.
use super::*;
impl IdentityStore {
    pub async fn android_record_private_sharing_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        project: Uuid,
        user: Uuid,
    ) -> Result<()> {
        self.require_global_session_in_transaction(tx, actor)
            .await?;
        self.require_session_action_in_transaction(tx, actor, ContentAction::Share)
            .await?;
        let allowed:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_projects p JOIN identity_subjects i ON i.id=p.owner_id WHERE p.id=$1 AND p.deleted=0 AND i.active=1 AND (p.owner_id=$2 OR EXISTS(SELECT 1 FROM android_private_project_grants g WHERE g.project_id=p.id AND g.owner_id=p.owner_id AND g.user_id=$2 AND g.active=1 AND (g.permissions&8)=8))").bind(project.to_string()).bind(actor.identity_id.to_string()).fetch_one(&mut **tx).await?;
        if allowed != 1 {
            return Err(IdentityError::Forbidden);
        }
        self.audit_event(
            tx,
            None,
            actor.identity_id,
            "private_project_access_changed",
            user,
        )
        .await?;
        self.require_global_session_in_transaction(tx, actor).await
    }
}

impl IdentityStore {
    pub async fn android_record_storage_quota_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        target: Uuid,
    ) -> Result<()> {
        self.require_global_session_in_transaction(tx, actor)
            .await?;
        self.require_session_action_in_transaction(tx, actor, ContentAction::Write)
            .await?;
        self.platform_admin(tx, actor).await?;
        self.audit_event(
            tx,
            None,
            actor.identity_id,
            "personal_storage_quota_changed",
            target,
        )
        .await?;
        self.platform_admin(tx, actor).await?;
        Ok(())
    }
}

impl IdentityStore {
    /// Validates only the original durable replication delegation. No session,
    /// bearer, Principal, content grant or execution permission is returned.
    pub async fn android_validate_executor_sync_source_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        executor: Uuid,
        project: Uuid,
    ) -> Result<()> {
        if !self.android_auth_security {
            return Err(IdentityError::Forbidden);
        }
        self.lock_installation(tx).await?;
        let r=sqlx::query("SELECT g.issuer_id,g.issuer_session_id,g.issuer_session_revision,g.permissions,g.organization_id FROM android_executor_sync_grants g JOIN android_devices d ON d.id=g.executor_id WHERE g.executor_id=$1 AND g.project_id=$2 AND g.active=1 AND d.revoked=0 AND d.identity_id=g.owner_id AND g.issuer_id=g.owner_id").bind(executor.to_string()).bind(project.to_string()).fetch_optional(&mut **tx).await?.ok_or(IdentityError::Forbidden)?;
        // The tuple comes solely from the previously authorized immutable SQL source.
        // It is never an HTTP/user-selected session and cannot escape this validator.
        let actual = Principal {
            identity_id: identifier(r.try_get("issuer_id")?)?,
            session_id: identifier(r.try_get("issuer_session_id")?)?,
            session_revision: r.try_get("issuer_session_revision")?,
        };
        self.actor(tx, &actual).await?;
        let scope = self.session_scope(tx, &actual).await?;
        let org: Option<String> = r.try_get("organization_id")?;
        if scope.is_some() && scope.map(|v| v.to_string()) != org {
            return Err(IdentityError::Forbidden);
        }
        let action = if r.try_get::<i64, _>("permissions")? == 3 {
            ContentAction::Write
        } else {
            ContentAction::Read
        };
        self.require_session_action_in_transaction(tx, &actual, action)
            .await?;
        let now = self.sample(tx).await?;
        let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_executor_sync_grants WHERE executor_id=$1 AND project_id=$2 AND issuer_id=$3 AND issuer_session_id=$4 AND issuer_session_revision=$5 AND active=1 AND expires_at>$6").bind(executor.to_string()).bind(project.to_string()).bind(actual.identity_id.to_string()).bind(actual.session_id.to_string()).bind(actual.session_revision).bind(now).fetch_one(&mut **tx).await?;
        if valid != 1 {
            return Err(IdentityError::Forbidden);
        }
        Ok(())
    }
}
