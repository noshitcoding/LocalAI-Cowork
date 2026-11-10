use super::*;
impl Store {
    pub async fn android_security_context(&self, actor: &Principal) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        authority(self, &mut tx, actor).await?;
        let value = json!({"schema_version":2,"user_id":actor.identity_id(),"session_id":actor.session_id()});
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_security_email(&self, actor: &Principal) -> Result<String> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let email = self
            .identity()
            .android_identity_email_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(email)
    }
    pub async fn android_security_desktop_reauthentication(
        &self,
        actor: &Principal,
        password: &str,
        token: &str,
    ) -> Result<Value> {
        if token.len() < 32 || token.len() > 200 || !token.is_ascii() {
            return Err(invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        authority(self, &mut tx, actor).await?;
        self.identity()
            .verify_current_password_in_transaction(&mut tx, actor, password)
            .await
            .map_err(identity_error)?;
        let revision:i64=sqlx::query_scalar("SELECT revision FROM identity_sessions WHERE id=$1 AND identity_id=$2 AND revoked=0 AND expires_at>$3").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(current).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let expiry = current.checked_add(60_000).ok_or(Error::Conflict)?;
        sqlx::query("INSERT INTO android_device_tickets(token_sha256,identity_id,session_id,session_revision,kind,payload,expires_at,consumed) VALUES($1,$2,$3,$4,'desktop_reauthentication','{}',$5,0)").bind(hash(token.as_bytes())).bind(actor.identity_id().to_string()).bind(actor.session_id().to_string()).bind(revision).bind(expiry).execute(&mut *tx).await.map_err(storage_error)?;
        audit(
            &mut tx,
            actor,
            "auth.desktop.reauthenticated",
            actor.session_id(),
            current,
        )
        .await?;
        authority(self, &mut tx, actor).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"schema_version":2,"token":token,"purpose":"desktop_control","expires_at":device_iso(expiry)?}),
        )
    }
    pub async fn android_security_sessions(&self, actor: &Principal) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        authority(self, &mut tx, actor).await?;
        let rows=sqlx::query("SELECT s.id,s.expires_at,s.revoked,m.device_id,m.created_at,m.last_used_at,m.revoked_at,m.revoke_reason FROM identity_sessions s JOIN android_auth_session_metadata m ON m.session_id=s.id AND m.owner_id=s.identity_id WHERE s.identity_id=$1 AND m.created_at>$2 ORDER BY CASE WHEN s.id=$3 THEN 0 ELSE 1 END,CASE WHEN s.revoked=0 AND s.expires_at>$4 THEN 0 ELSE 1 END,m.last_used_at DESC LIMIT 100")
            .bind(actor.identity_id().to_string()).bind(current.saturating_sub(90*24*60*60*1000)).bind(actor.session_id().to_string()).bind(current).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let mut values = Vec::new();
        for r in rows {
            let sid = uuid(&r.try_get::<String, _>("id").map_err(storage_error)?)?;
            let device: String = r.try_get("device_id").map_err(storage_error)?;
            // The original V1 wire requires a device UUID. Actual native/global
            // sessions without that provenance remain visible through the V2
            // account API; never invent a device or adopt a historical one.
            let Ok(device) = Uuid::parse_str(&device) else {
                continue;
            };
            if device.is_nil() {
                continue;
            }
            let expires: i64 = r.try_get("expires_at").map_err(storage_error)?;
            let revoked = r.try_get::<i64, _>("revoked").map_err(storage_error)? != 0;
            let at: i64 = r.try_get("revoked_at").map_err(storage_error)?;
            let reason: String = r.try_get("revoke_reason").map_err(storage_error)?;
            values.push(json!({"schema_version":2,"id":sid,"device_id":device,"current":sid==actor.session_id(),"active":!revoked&&expires>current,"created_at":device_iso(r.try_get("created_at").map_err(storage_error)?)?,"last_used_at":device_iso(r.try_get("last_used_at").map_err(storage_error)?)?,"expires_at":device_iso(expires)?,"revoked_at":if at>0{Some(device_iso(at)?)}else{None},"revoke_reason":if reason.is_empty(){None}else{Some(reason)}}));
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_security_revoke_session(
        &self,
        actor: &Principal,
        session: Uuid,
    ) -> Result<()> {
        if session.is_nil() {
            return Err(invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        authority(self, &mut tx, actor).await?;
        let row = sqlx::query(
            "SELECT revision,revoked FROM identity_sessions WHERE id=$1 AND identity_id=$2",
        )
        .bind(session.to_string())
        .bind(actor.identity_id().to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        if row.try_get::<i64, _>("revoked").map_err(storage_error)? == 0 {
            one(sqlx::query("UPDATE identity_sessions SET revoked=1,revision=revision+1 WHERE id=$1 AND identity_id=$2 AND revision=$3 AND revoked=0").bind(session.to_string()).bind(actor.identity_id().to_string()).bind(row.try_get::<i64,_>("revision").map_err(storage_error)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected())?;
            sqlx::query("UPDATE android_auth_session_metadata SET revoked_at=$1,revoke_reason='user_device_revoked' WHERE session_id=$2 AND owner_id=$3 AND revoked_at=0").bind(current).bind(session.to_string()).bind(actor.identity_id().to_string()).execute(&mut *tx).await.map_err(storage_error)?;
            audit(&mut tx, actor, "auth.session.revoked", session, current).await?;
        }
        // Every bearer/refresh grant ultimately resolves this real session.
        // No grant can issue a replacement after its identity session is revoked.
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
}
