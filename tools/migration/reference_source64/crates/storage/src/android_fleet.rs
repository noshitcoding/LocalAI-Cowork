//! Same-transaction consumption of a verified, instance-bound federation nonce.
use super::*;
use crate::android_devices::device_writer;
use team_service::identity_error;
impl Store {
    pub async fn android_fleet_session(
        &self,
        identity: Uuid,
        team: Uuid,
        email: &str,
        nonce: Uuid,
        expires: i64,
        viewer: bool,
        support: bool,
    ) -> Result<cowork_identity::SessionIssued> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        if expires <= current || expires > current + 90_000 {
            return Err(Error::Forbidden);
        }
        let row=sqlx::query("SELECT i.username,m.role FROM identity_subjects i JOIN identity_memberships m ON m.identity_id=i.id JOIN identity_organizations o ON o.id=m.organization_id WHERE i.id=$1 AND i.active=1 AND m.organization_id=$2 AND m.active=1 AND o.state='active'").bind(identity.to_string()).bind(team.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        if row
            .try_get::<String, _>("username")
            .map_err(storage_error)?
            != email.trim().to_lowercase()
            || support
                && !matches!(
                    row.try_get::<String, _>("role")
                        .map_err(storage_error)?
                        .as_str(),
                    "owner" | "admin"
                )
        {
            return Err(Error::Forbidden);
        }
        let issued = self
            .identity()
            .issue_verified_scoped_session_with_permissions_in_transaction(
                &mut tx,
                identity,
                team,
                8 * 60 * 60_000,
                if viewer { 1 } else { 15 },
            )
            .await
            .map_err(identity_error)?;
        // Persist the issuer nonce, not the signed bearer. UNIQUE prevents replay.
        sqlx::query("INSERT INTO android_fleet_nonces(nonce_sha256,identity_id,organization_id,expires_at) VALUES($1,$2,$3,$4)").bind(hash(format!("fleet:{team}:{nonce}").as_bytes())).bind(identity.to_string()).bind(team.to_string()).bind(expires).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(issued)
    }
}
