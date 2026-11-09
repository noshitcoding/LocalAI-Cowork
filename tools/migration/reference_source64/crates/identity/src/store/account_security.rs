use super::*;

impl IdentityStore {
    /// Public session metadata never contains credential hashes or bearer values.
    pub async fn account_sessions(&self, actor: &Principal) -> Result<Vec<AccountSession>> {
        let mut tx = self.begin().await?;
        self.require_global_session_in_transaction(&mut tx, actor)
            .await?;
        self.actor(&mut tx, actor).await?;
        self.require_android_management_in_transaction(&mut tx, actor)
            .await?;
        let rows = sqlx::query("SELECT id,expires_at,revoked,revision FROM identity_sessions WHERE identity_id=$1 ORDER BY expires_at DESC,id LIMIT 500")
            .bind(actor.identity_id.to_string()).fetch_all(&mut *tx).await?;
        let mut result = Vec::new();
        for row in rows {
            let id = identifier(row.try_get("id")?)?;
            result.push(AccountSession {
                id,
                current: id == actor.session_id,
                expires_at: row.try_get("expires_at")?,
                revoked: row.try_get::<i64, _>("revoked")? != 0,
                revision: row.try_get("revision")?,
            });
        }
        tx.commit().await?;
        Ok(result)
    }
    pub async fn revoke_account_session(
        &self,
        actor: &Principal,
        session: Uuid,
        revision: i64,
    ) -> Result<()> {
        let mut tx = self.begin().await?;
        self.require_global_session_in_transaction(&mut tx, actor)
            .await?;
        self.actor(&mut tx, actor).await?;
        self.require_android_management_in_transaction(&mut tx, actor)
            .await?;
        let row = sqlx::query(
            "SELECT revision,revoked FROM identity_sessions WHERE id=$1 AND identity_id=$2",
        )
        .bind(session.to_string())
        .bind(actor.identity_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(IdentityError::NotFound)?;
        if row.try_get::<i64, _>("revision")? != revision || row.try_get::<i64, _>("revoked")? != 0
        {
            return Err(IdentityError::Conflict);
        }
        let changed = sqlx::query("UPDATE identity_sessions SET revoked=1,revision=revision+1 WHERE id=$1 AND identity_id=$2 AND revision=$3 AND revoked=0")
            .bind(session.to_string()).bind(actor.identity_id.to_string()).bind(revision).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(IdentityError::Conflict);
        }
        self.revoke_android_metadata(&mut tx, actor.identity_id, "user_device_revoked")
            .await?;
        self.audit_event(&mut tx, None, actor.identity_id, "session_revoked", session)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn account_audit(
        &self,
        actor: &Principal,
        after: i64,
        limit: i64,
    ) -> Result<Vec<AuditEntry>> {
        if after < 0 || !(1..=500).contains(&limit) {
            return Err(IdentityError::Validation);
        }
        let mut tx = self.begin().await?;
        self.require_global_session_in_transaction(&mut tx, actor)
            .await?;
        self.actor(&mut tx, actor).await?;
        self.require_android_management_in_transaction(&mut tx, actor)
            .await?;
        let rows = sqlx::query("SELECT * FROM identity_audit WHERE organization_id IS NULL AND (actor_id=$1 OR target_id=$1) AND sequence>$2 ORDER BY sequence LIMIT $3")
            .bind(actor.identity_id.to_string()).bind(after).bind(limit).fetch_all(&mut *tx).await?;
        let mut result = Vec::new();
        for row in rows {
            result.push(AuditEntry {
                sequence: row.try_get("sequence")?,
                id: identifier(row.try_get("id")?)?,
                organization_id: None,
                actor_id: identifier(row.try_get("actor_id")?)?,
                action: row.try_get("action")?,
                target_id: identifier(row.try_get("target_id")?)?,
                observed_at: row.try_get("observed_at")?,
            });
        }
        tx.commit().await?;
        Ok(result)
    }
    pub async fn account_directory(
        &self,
        actor: &Principal,
        query: &str,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<AccountDirectoryPage> {
        if query.chars().count() > 200
            || query.chars().any(char::is_control)
            || !(1..=500).contains(&limit)
        {
            return Err(IdentityError::Validation);
        }
        let mut tx = self.begin().await?;
        self.platform_admin(&mut tx, actor).await?;
        let pattern = format!(
            "%{}%",
            query
                .trim()
                .to_lowercase()
                .replace('!', "!!")
                .replace('%', "!%")
                .replace('_', "!_")
        );
        let rows = sqlx::query("SELECT * FROM identity_subjects WHERE id>$1 AND (LOWER(username) LIKE $2 ESCAPE '!' OR LOWER(display_name) LIKE $2 ESCAPE '!') ORDER BY id LIMIT $3")
            .bind(after.map(|id|id.to_string()).unwrap_or_default()).bind(pattern).bind(i64::from(limit)+1)
            .fetch_all(&mut *tx)
            .await?;
        let mut items = rows.iter().map(identity).collect::<Result<Vec<_>>>()?;
        let has_more = items.len() > limit as usize;
        items.truncate(limit as usize);
        let next_cursor = has_more.then(|| items.last().map(|i| i.id)).flatten();
        tx.commit().await?;
        Ok(AccountDirectoryPage {
            items,
            next_cursor,
            has_more,
        })
    }
}
