//! Authentication adapters rotate only the actual authenticated session, in the caller's writer.
use super::*;
impl IdentityStore {
    pub async fn rotate_android_session_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        ttl: i64,
    ) -> Result<SessionIssued> {
        self.lock_installation(tx).await?;
        let identity = self.actor(tx, actor).await?;
        let scope = self.session_scope(tx, actor).await?;
        let ceiling = self.session_scope_permissions(tx, actor).await?;
        let issued = self.session(tx, identity, ttl).await?;
        if self.android_auth_security {
            self.inherit_android_assurance(tx, actor, &issued).await?;
        }
        if let Some(org) = scope {
            self.bind_session_scope(tx, &issued, org, ceiling).await?;
        }
        if sqlx::query("UPDATE identity_sessions SET revoked=1,revision=revision+1 WHERE id=$1 AND identity_id=$2 AND revoked=0 AND revision=$3")
            .bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(actor.session_revision)
            .execute(&mut **tx).await?.rows_affected()!=1 { return Err(IdentityError::Conflict); }
        if self.android_auth_security {
            let current = self.sample(tx).await?;
            sqlx::query("UPDATE android_auth_session_metadata SET revoked_at=$1,revoke_reason='session_rotated' WHERE session_id=$2 AND owner_id=$3 AND revoked_at=0")
                .bind(current).bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).execute(&mut **tx).await?;
        }
        self.audit_event(
            tx,
            None,
            actor.identity_id(),
            "session_rotated",
            actor.session_id(),
        )
        .await?;
        Ok(issued)
    }
}
