//! The persistent operator policy is checked under the same installation writer as the effect.
use super::*;
impl IdentityStore {
    async fn android_management_verified(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        current: i64,
    ) -> Result<bool> {
        let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_session_metadata m WHERE m.session_id=$1 AND m.owner_id=$2 AND m.revoked_at=0 AND m.verified_at<=$3 AND m.verified_at>$4 AND ((m.factor_method IN ('totp','recovery') AND EXISTS(SELECT 1 FROM android_auth_totp t WHERE t.owner_id=m.owner_id AND t.enabled=1 AND t.revision=m.factor_revision)) OR (m.factor_method='passkey' AND EXISTS(SELECT 1 FROM android_auth_passkey_proofs p JOIN android_passkeys k ON k.id=p.passkey_id AND k.identity_id=p.owner_id WHERE p.session_id=m.session_id AND p.owner_id=m.owner_id AND p.credential_revision=m.factor_revision AND k.revision>=p.credential_revision AND p.verified_at=m.verified_at)))")
            .bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(current).bind(current.saturating_sub(43_200_000)).fetch_one(&mut **tx).await?;
        Ok(n == 1)
    }
    pub async fn require_android_management_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
    ) -> Result<()> {
        self.lock_installation(tx).await?;
        let actual = self.actor(tx, actor).await?;
        if !self.android_auth_security || !actual.platform_admin {
            return Ok(());
        }
        let required: i64 = sqlx::query_scalar(
            "SELECT admin_required FROM android_auth_security_policy WHERE id=1",
        )
        .fetch_one(&mut **tx)
        .await?;
        if required != 1 {
            return Ok(());
        }
        let current = self.sample(tx).await?;
        if !self.android_management_verified(tx, actor, current).await? {
            return Err(IdentityError::Forbidden);
        }
        Ok(())
    }
    pub async fn android_management_request(
        &self,
        actor: &Principal,
        method: &str,
        path: &str,
    ) -> Result<()> {
        let mut tx = self.begin().await?;
        let actual = self.actor(&mut tx, actor).await?;
        if !self.android_auth_security || !actual.platform_admin {
            tx.commit().await?;
            return Ok(());
        }
        let required: i64 = sqlx::query_scalar(
            "SELECT admin_required FROM android_auth_security_policy WHERE id=1",
        )
        .fetch_one(&mut *tx)
        .await?;
        if required != 1 {
            tx.commit().await?;
            return Ok(());
        }
        let current = self.sample(&mut tx).await?;
        let verified = self
            .android_management_verified(&mut tx, actor, current)
            .await?;
        let path = path
            .strip_prefix("/api/v1")
            .or_else(|| path.strip_prefix("/api/v2/identity"))
            .unwrap_or(path);
        let recovery = matches!(
            (method, path),
            (
                "GET",
                "/auth/assurance" | "/auth/security-context" | "/auth/totp" | "/auth/me" | "/me"
            ) | (
                "POST",
                "/auth/assurance/verify" | "/auth/logout" | "/logout"
            )
        );
        let recent:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_session_metadata m WHERE m.session_id=$1 AND m.owner_id=$2 AND m.created_at<=$3 AND m.created_at>$4 AND m.revoked_at=0 AND NOT EXISTS(SELECT 1 FROM android_auth_totp t WHERE t.owner_id=m.owner_id AND t.enabled=1) AND NOT EXISTS(SELECT 1 FROM android_passkeys p WHERE p.identity_id=m.owner_id)")
            .bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(current).bind(current.saturating_sub(600_000)).fetch_one(&mut *tx).await?;
        let enrollment = recent == 1
            && matches!(
                (method, path),
                ("POST", "/auth/totp/setup" | "/auth/totp/enable")
            );
        if !verified && !recovery && !enrollment {
            return Err(IdentityError::Forbidden);
        }
        tx.commit().await?;
        Ok(())
    }
}
