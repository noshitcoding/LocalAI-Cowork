use super::*;
impl IdentityStore {
    pub(in crate::store) async fn validate_oidc_origin_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
    ) -> Result<()> {
        let Some(row) =
            sqlx::query("SELECT * FROM android_oidc_session_origins WHERE session_id=$1")
                .bind(actor.session_id.to_string())
                .fetch_optional(&mut **tx)
                .await?
        else {
            return Ok(());
        };
        let p = profiles::read(tx, identifier(row.try_get("profile_id")?)?).await?;
        let current = self.sample(tx).await?;
        if let Some(policy) = sso::admission(tx, &p).await? {
            if policy.revision != row.try_get::<i64, _>("sso_revision")?
                || Some(policy.epoch.to_string())
                    != row.try_get::<Option<String>, _>("sso_epoch")?
            {
                return Err(IdentityError::Unauthenticated);
            }
        }
        if row.try_get::<String, _>("identity_id")? != actor.identity_id.to_string()
            || !p.active
            || p.revision != row.try_get::<i64, _>("profile_revision")?
            || p.revision != row.try_get::<i64, _>("source_grant_revision")?
            || p.epoch.to_string() != row.try_get::<String, _>("profile_epoch")?
            || p.config_sha256 != row.try_get::<String, _>("config_sha256")?
            || row.try_get::<i64, _>("expires_at")? <= current
            || row.try_get::<i64, _>("verified_at")? > current
            || profiles::org_epoch(tx, p.organization_id).await?
                != row.try_get::<i64, _>("organization_epoch")?
        {
            return Err(IdentityError::Unauthenticated);
        }
        let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_oidc_external_identities e JOIN android_oidc_flows f ON f.id=$1 JOIN android_oidc_receipts r ON r.flow_id=f.id WHERE e.id=$2 AND e.identity_id=$3 AND e.profile_id=$4 AND e.issuer=$5 AND e.revision=$6 AND e.active=1 AND f.profile_revision=$7 AND f.profile_epoch=$8 AND f.phase='committed' AND r.external_identity_id=e.id AND r.identity_id=e.identity_id").bind(row.try_get::<String,_>("flow_id")?).bind(row.try_get::<String,_>("external_identity_id")?).bind(actor.identity_id.to_string()).bind(p.id.to_string()).bind(&p.config.issuer).bind(row.try_get::<i64,_>("external_revision")?).bind(p.revision).bind(p.epoch.to_string()).fetch_one(&mut **tx).await?;
        if valid != 1 {
            return Err(IdentityError::Unauthenticated);
        }
        let current = self.sample(tx).await?;
        if row.try_get::<i64, _>("expires_at")? <= current {
            return Err(IdentityError::Unauthenticated);
        }
        Ok(())
    }
    pub(in crate::store) async fn inherit_oidc_origin_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        issued: &SessionIssued,
    ) -> Result<()> {
        self.validate_oidc_origin_in_transaction(tx, actor).await?;
        let Some(source)=sqlx::query("SELECT expires_at FROM android_oidc_session_origins WHERE session_id=$1 AND identity_id=$2").bind(actor.session_id.to_string()).bind(actor.identity_id.to_string()).fetch_optional(&mut **tx).await?else{return Ok(())};
        let target:String=sqlx::query_scalar("SELECT id FROM identity_sessions WHERE identity_id=$1 AND token_sha256=$2 AND revoked=0").bind(actor.identity_id.to_string()).bind(secret_hash("session-v1",&issued.token)).fetch_one(&mut **tx).await?;
        sqlx::query("INSERT INTO android_oidc_session_origins(session_id,identity_id,profile_id,external_identity_id,external_revision,profile_revision,profile_epoch,config_sha256,organization_epoch,source_grant_revision,flow_id,claims_sha256,verified_at,authentication_time,expires_at,sso_revision,sso_epoch) SELECT $1,identity_id,profile_id,external_identity_id,external_revision,profile_revision,profile_epoch,config_sha256,organization_epoch,source_grant_revision,flow_id,claims_sha256,verified_at,authentication_time,expires_at,sso_revision,sso_epoch FROM android_oidc_session_origins WHERE session_id=$2 AND identity_id=$3").bind(&target).bind(actor.session_id.to_string()).bind(actor.identity_id.to_string()).execute(&mut **tx).await?;
        sqlx::query("UPDATE identity_sessions SET expires_at=CASE WHEN expires_at>$1 THEN $1 ELSE expires_at END WHERE id=$2 AND identity_id=$3").bind(source.try_get::<i64,_>("expires_at")?).bind(target).bind(actor.identity_id.to_string()).execute(&mut **tx).await?;
        self.validate_oidc_origin_in_transaction(tx, actor).await?;
        Ok(())
    }
    pub async fn android_oidc_origin(&self, a: &Principal) -> Result<Option<AndroidOidcOrigin>> {
        let mut tx = self.begin().await?;
        let value = self.android_oidc_origin_in_transaction(&mut tx, a).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn android_oidc_origin_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        a: &Principal,
    ) -> Result<Option<AndroidOidcOrigin>> {
        self.actor(tx, a).await?;
        let row = sqlx::query(
            "SELECT * FROM android_oidc_session_origins WHERE session_id=$1 AND identity_id=$2",
        )
        .bind(a.session_id.to_string())
        .bind(a.identity_id.to_string())
        .fetch_optional(&mut **tx)
        .await?;
        let value = if let Some(row) = row {
            let p = profiles::read(tx, identifier(row.try_get("profile_id")?)?).await?;
            Some(AndroidOidcOrigin {
                method: "oidc_signed_subject".into(),
                profile_id: p.id,
                profile_revision: p.revision,
                organization_id: p.organization_id,
                external_identity_id: identifier(row.try_get("external_identity_id")?)?,
                issuer: p.config.issuer,
                verified_at: row.try_get("verified_at")?,
                expires_at: row.try_get("expires_at")?,
                claims_sha256: row.try_get("claims_sha256")?,
                flow_id: identifier(row.try_get("flow_id")?)?,
            })
        } else {
            None
        };
        self.actor(tx, a).await?;
        Ok(value)
    }
}
