//! Tenant-bound sessions from an explicitly configured federation issuer.
use super::*;
pub(crate) const SESSION_SCOPE_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS identity_session_scopes (session_id TEXT PRIMARY KEY REFERENCES identity_sessions(id), organization_id TEXT NOT NULL REFERENCES identity_organizations(id), permissions BIGINT NOT NULL CHECK(permissions IN (1,15)))";
impl IdentityStore {
    pub(super) async fn session_scope(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
    ) -> Result<Option<Uuid>> {
        let row =
            sqlx::query("SELECT organization_id FROM identity_session_scopes WHERE session_id=$1")
                .bind(actor.session_id.to_string())
                .fetch_optional(&mut **tx)
                .await?;
        row.map(|r| identifier(r.try_get("organization_id")?))
            .transpose()
    }
    pub(super) async fn session_scope_permissions(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
    ) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT permissions FROM identity_session_scopes WHERE session_id=$1",
        )
        .bind(actor.session_id.to_string())
        .fetch_optional(&mut **tx)
        .await?
        .unwrap_or(15))
    }
    pub async fn require_session_action_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        action: ContentAction,
    ) -> Result<()> {
        self.lock_installation(tx).await?;
        self.actor(tx, actor).await?;
        self.require_android_management_in_transaction(tx, actor)
            .await?;
        if self.session_scope_permissions(tx, actor).await? & action.bit() == 0 {
            return Err(IdentityError::Forbidden);
        }
        Ok(())
    }
    /// Identity-scoped enrollment must never turn a federated team login into
    /// a reusable global passkey, bearer, or platform administration credential.
    pub async fn require_global_session_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
    ) -> Result<()> {
        self.lock_installation(tx).await?;
        self.actor(tx, actor).await?;
        if self.session_scope(tx, actor).await?.is_some() {
            return Err(IdentityError::Forbidden);
        }
        Ok(())
    }
    pub async fn session_organization_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
    ) -> Result<Option<Uuid>> {
        self.lock_installation(tx).await?;
        self.actor(tx, actor).await?;
        self.session_scope(tx, actor).await
    }
    /// Exposes the effective read-only ceiling for current-session UI metadata.
    pub async fn session_is_read_only_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
    ) -> Result<bool> {
        self.lock_installation(tx).await?;
        self.actor(tx, actor).await?;
        Ok(self.session_scope_permissions(tx, actor).await? == 1)
    }
    pub(super) async fn bind_session_scope(
        &self,
        tx: &mut Transaction<'_, Any>,
        issued: &SessionIssued,
        org: Uuid,
        permissions: i64,
    ) -> Result<()> {
        let session_id:String=sqlx::query_scalar("SELECT id FROM identity_sessions WHERE token_sha256=$1 AND identity_id=$2 AND revoked=0")
            .bind(secret_hash("session-v1",&issued.token)).bind(issued.identity.id.to_string()).fetch_one(&mut **tx).await?;
        sqlx::query("INSERT INTO identity_session_scopes(session_id,organization_id,permissions) VALUES($1,$2,$3)")
            .bind(session_id).bind(org.to_string()).bind(permissions).execute(&mut **tx).await?;
        Ok(())
    }
    /// Server-internal verified federation hook. Caller verifies its signed
    /// issuer/instance/user/expiry/nonce and consumes the nonce in this same TX.
    pub async fn issue_verified_scoped_session_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        identity_id: Uuid,
        org: Uuid,
        ttl_millis: i64,
    ) -> Result<SessionIssued> {
        self.issue_verified_scoped_session_with_permissions_in_transaction(
            tx,
            identity_id,
            org,
            ttl_millis,
            15,
        )
        .await
    }
    pub async fn issue_verified_scoped_session_with_permissions_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        identity_id: Uuid,
        org: Uuid,
        ttl_millis: i64,
        permissions: i64,
    ) -> Result<SessionIssued> {
        if !matches!(permissions, 1 | 15) {
            return Err(IdentityError::Validation);
        }
        self.lock_installation(tx).await?;
        let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM identity_memberships m JOIN identity_organizations o ON o.id=m.organization_id JOIN identity_subjects i ON i.id=m.identity_id WHERE m.identity_id=$1 AND m.organization_id=$2 AND m.active=1 AND o.state='active' AND i.active=1")
            .bind(identity_id.to_string()).bind(org.to_string()).fetch_one(&mut **tx).await?;
        if active != 1 {
            return Err(IdentityError::Forbidden);
        }
        let mut issued = self
            .issue_verified_session_in_transaction(tx, identity_id, ttl_millis)
            .await?;
        self.bind_session_scope(tx, &issued, org, permissions)
            .await?;
        issued.identity.platform_admin = false;
        self.audit_event(
            tx,
            Some(org),
            identity_id,
            "scoped_verified_session_issued",
            identity_id,
        )
        .await?;
        Ok(issued)
    }
}
