use super::*;
pub(super) async fn read(tx: &mut Transaction<'_, Any>, id: Uuid) -> Result<AndroidOidcProfile> {
    let row = sqlx::query("SELECT * FROM android_oidc_profiles WHERE id=$1")
        .bind(id.to_string())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(IdentityError::NotFound)?;
    let config: AndroidOidcConfig = decoded(&row.try_get::<String, _>("config")?)?;
    super::config(&config)?;
    let sha = digest(encoded(&config)?.as_bytes());
    if sha != row.try_get::<String, _>("config_sha256")? {
        return Err(IdentityError::Corrupt);
    }
    Ok(AndroidOidcProfile {
        id,
        organization_id: row
            .try_get::<Option<String>, _>("organization_id")?
            .map(identifier)
            .transpose()?,
        name: row.try_get("name")?,
        revision: row.try_get("revision")?,
        epoch: identifier(row.try_get("epoch")?)?,
        config_sha256: sha,
        config,
        active: row.try_get::<i64, _>("active")? == 1,
        has_secret: row.try_get::<Option<String>, _>("secret_id")?.is_some(),
    })
}
pub(super) async fn org_epoch(tx: &mut Transaction<'_, Any>, id: Option<Uuid>) -> Result<i64> {
    match id {
        Some(id) => sqlx::query_scalar(
            "SELECT authorization_epoch FROM identity_organizations WHERE id=$1 AND state='active'",
        )
        .bind(id.to_string())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(IdentityError::Forbidden),
        None => Ok(0),
    }
}
impl IdentityStore {
    pub(super) async fn manage_oidc(
        &self,
        tx: &mut Transaction<'_, Any>,
        a: &Principal,
        org: Option<Uuid>,
    ) -> Result<()> {
        if org.is_none() {
            self.require_global_session_in_transaction(tx, a).await?;
        }
        self.require_android_management_in_transaction(tx, a)
            .await?;
        self.require_android_factor_verified(tx, a).await?;
        if let Some(org) = org {
            let (_, member) = self.org_member(tx, a, org).await?;
            if !member.role.manages_members() {
                return Err(IdentityError::Forbidden);
            }
        } else {
            self.platform_admin(tx, a).await?;
        }
        Ok(())
    }
    pub async fn android_oidc_profiles(
        &self,
        a: Option<&Principal>,
        organization: Option<Uuid>,
        management: bool,
    ) -> Result<Vec<AndroidOidcProfile>> {
        let mut tx = self.begin().await?;
        if management {
            self.manage_oidc(
                &mut tx,
                a.ok_or(IdentityError::Unauthenticated)?,
                organization,
            )
            .await?;
        } else {
            org_epoch(&mut tx, organization).await?;
        }
        let rows=sqlx::query("SELECT id FROM android_oidc_profiles WHERE (organization_id=$1 OR (organization_id IS NULL AND $1 IS NULL)) AND (active=1 OR $2=1) ORDER BY id LIMIT 1001").bind(organization.map(|v|v.to_string())).bind(i64::from(management)).fetch_all(&mut *tx).await?;
        if rows.len() > 1000 {
            return Err(IdentityError::QuotaExceeded);
        }
        let mut out = Vec::new();
        for row in rows {
            out.push(read(&mut tx, identifier(row.try_get("id")?)?).await?)
        }
        if management {
            self.manage_oidc(
                &mut tx,
                a.ok_or(IdentityError::Unauthenticated)?,
                organization,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(out)
    }
    pub async fn android_oidc_profile_write(
        &self,
        a: &Principal,
        id: Option<Uuid>,
        q: &AndroidOidcProfileWrite,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<AndroidOidcProfile> {
        config(&q.config)?;
        if q.request_id.is_nil()
            || q.expected_revision < 0
            || id.is_none() && q.expected_revision != 0
            || !valid_name(&q.name)
            || q.name.len() > 200
            || q.client_secret
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 4096 || v.chars().any(char::is_control))
        {
            return Err(IdentityError::Validation);
        }
        let fingerprint = mac(keys, key, &(id, q))?;
        let mut tx = self.begin().await?;
        self.manage_oidc(&mut tx, a, q.organization_id).await?;
        if let Some(op) = sqlx::query(
            "SELECT * FROM android_oidc_profile_operations WHERE owner_id=$1 AND request_id=$2",
        )
        .bind(a.identity_id.to_string())
        .bind(q.request_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        {
            if op.try_get::<String, _>("request_mac")? != fingerprint {
                return Err(IdentityError::Conflict);
            }
            let bytes = cipher::read(
                &mut tx,
                keys,
                identifier(op.try_get("receipt_id")?)?,
                identifier(op.try_get("profile_id")?)?,
                "oauth_response",
            )
            .await?;
            let out: AndroidOidcProfile =
                serde_json::from_slice(&bytes).map_err(|_| IdentityError::Corrupt)?;
            self.manage_oidc(&mut tx, a, q.organization_id).await?;
            tx.commit().await?;
            return Ok(out);
        }
        let old = match id {
            Some(id) => Some(read(&mut tx, id).await?),
            None => None,
        };
        if old.as_ref().is_some_and(|p| {
            p.revision != q.expected_revision || p.organization_id != q.organization_id
        }) {
            return Err(IdentityError::Conflict);
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM android_oidc_profiles")
            .fetch_one(&mut *tx)
            .await?;
        if old.is_none() && count >= 1000 {
            return Err(IdentityError::QuotaExceeded);
        }
        let at = self.sample(&mut tx).await?;
        let p = AndroidOidcProfile {
            id: id.unwrap_or_else(Uuid::new_v4),
            organization_id: q.organization_id,
            name: q.name.trim().into(),
            revision: q
                .expected_revision
                .checked_add(1)
                .ok_or(IdentityError::Validation)?,
            epoch: Uuid::new_v4(),
            config_sha256: digest(encoded(&q.config)?.as_bytes()),
            config: q.config.clone(),
            active: q.active,
            has_secret: q.client_secret.is_some(),
        };
        let secret = match &q.client_secret {
            Some(v) => Some(
                cipher::put(
                    &mut tx,
                    keys,
                    key,
                    &p,
                    Uuid::new_v4(),
                    "oauth_profile",
                    v.as_bytes(),
                    at,
                )
                .await?,
            ),
            None => None,
        };
        let receipt = cipher::put(
            &mut tx,
            keys,
            key,
            &p,
            Uuid::new_v4(),
            "oauth_response",
            &serde_json::to_vec(&p).map_err(|_| IdentityError::Corrupt)?,
            at,
        )
        .await?;
        self.manage_oidc(&mut tx, a, q.organization_id).await?;
        if old.is_some() {
            if sqlx::query("UPDATE android_oidc_profiles SET name=$1,revision=$2,epoch=$3,config_sha256=$4,config=$5,active=$6,secret_id=$7 WHERE id=$8 AND revision=$9").bind(&p.name).bind(p.revision).bind(p.epoch.to_string()).bind(&p.config_sha256).bind(encoded(&p.config)?).bind(i64::from(p.active)).bind(secret.map(|v|v.to_string())).bind(p.id.to_string()).bind(q.expected_revision).execute(&mut *tx).await?.rows_affected()!=1{return Err(IdentityError::Conflict)}
        } else {
            sqlx::query("INSERT INTO android_oidc_profiles(id,organization_id,name,revision,epoch,config_sha256,config,active,secret_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(p.id.to_string()).bind(p.organization_id.map(|v|v.to_string())).bind(&p.name).bind(p.revision).bind(p.epoch.to_string()).bind(&p.config_sha256).bind(encoded(&p.config)?).bind(i64::from(p.active)).bind(secret.map(|v|v.to_string())).execute(&mut *tx).await?;
        }
        sso::revoke_profile(&mut tx, p.id).await?;
        sqlx::query("UPDATE identity_sessions SET revoked=1,revision=revision+1 WHERE id IN(SELECT session_id FROM android_oidc_session_origins WHERE profile_id=$1) AND revoked=0").bind(p.id.to_string()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO android_oidc_profile_operations(owner_id,request_id,profile_id,request_mac,receipt_id) VALUES($1,$2,$3,$4,$5)").bind(a.identity_id.to_string()).bind(q.request_id.to_string()).bind(p.id.to_string()).bind(fingerprint).bind(receipt.to_string()).execute(&mut *tx).await?;
        self.audit_event(
            &mut tx,
            p.organization_id,
            a.identity_id,
            "oidc_profile_changed",
            p.id,
        )
        .await?;
        tx.commit().await?;
        Ok(p)
    }
    pub async fn android_oidc_links(&self, a: &Principal) -> Result<Vec<AndroidOidcLink>> {
        let mut tx = self.begin().await?;
        self.actor(&mut tx, a).await?;
        let rows=sqlx::query("SELECT * FROM android_oidc_external_identities WHERE identity_id=$1 ORDER BY id LIMIT 1001").bind(a.identity_id.to_string()).fetch_all(&mut *tx).await?;
        if rows.len() > 1000 {
            return Err(IdentityError::QuotaExceeded);
        }
        let mut result = Vec::new();
        for row in rows {
            result.push(AndroidOidcLink {
                id: identifier(row.try_get("id")?)?,
                profile_id: identifier(row.try_get("profile_id")?)?,
                issuer: row.try_get("issuer")?,
                subject: row.try_get("subject")?,
                revision: row.try_get("revision")?,
                active: row.try_get::<i64, _>("active")? == 1,
            })
        }
        self.actor(&mut tx, a).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn android_oidc_unlink(
        &self,
        a: &Principal,
        id: Uuid,
        q: &cowork_contracts::AndroidOidcUnlink,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<Value> {
        if q.request_id.is_nil() || q.expected_revision < 1 {
            return Err(IdentityError::Validation);
        }
        let next_revision = q
            .expected_revision
            .checked_add(1)
            .ok_or(IdentityError::Validation)?;
        let mut tx = self.begin().await?;
        self.require_global_session_in_transaction(&mut tx, a)
            .await?;
        self.require_android_factor_verified(&mut tx, a).await?;
        let fingerprint = mac(keys, key, &("unlink", id, q))?;
        if let Some(op) = sqlx::query(
            "SELECT * FROM android_oidc_profile_operations WHERE owner_id=$1 AND request_id=$2",
        )
        .bind(a.identity_id.to_string())
        .bind(q.request_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        {
            if op.try_get::<String, _>("request_mac")? != fingerprint {
                return Err(IdentityError::Conflict);
            }
            let result: Value = serde_json::from_slice(
                &cipher::read(
                    &mut tx,
                    keys,
                    identifier(op.try_get("receipt_id")?)?,
                    identifier(op.try_get("profile_id")?)?,
                    "oauth_response",
                )
                .await?,
            )
            .map_err(|_| IdentityError::Corrupt)?;
            self.actor(&mut tx, a).await?;
            tx.commit().await?;
            return Ok(result);
        }
        let external=sqlx::query("SELECT * FROM android_oidc_external_identities WHERE id=$1 AND identity_id=$2 AND active=1 AND revision=$3").bind(id.to_string()).bind(a.identity_id.to_string()).bind(q.expected_revision).fetch_optional(&mut *tx).await?.ok_or(IdentityError::Conflict)?;
        let profile = read(&mut tx, identifier(external.try_get("profile_id")?)?).await?;
        let result = json!({"external_identity_id":id,"revision":next_revision,"revoked":true,"request_id":q.request_id});
        let at = self.sample(&mut tx).await?;
        let receipt = cipher::put(
            &mut tx,
            keys,
            key,
            &profile,
            Uuid::new_v4(),
            "oauth_response",
            &serde_json::to_vec(&result).map_err(|_| IdentityError::Corrupt)?,
            at,
        )
        .await?;
        self.actor(&mut tx, a).await?;
        self.require_android_factor_verified(&mut tx, a).await?;
        if sqlx::query("UPDATE android_oidc_external_identities SET active=0,revision=revision+1 WHERE id=$1 AND identity_id=$2 AND revision=$3 AND active=1").bind(id.to_string()).bind(a.identity_id.to_string()).bind(q.expected_revision).execute(&mut *tx).await?.rows_affected() != 1 { return Err(IdentityError::Conflict); }
        sso::revoke_external(&mut tx, id).await?;
        sqlx::query("UPDATE identity_sessions SET revoked=1,revision=revision+1 WHERE id IN(SELECT session_id FROM android_oidc_session_origins WHERE external_identity_id=$1) AND revoked=0").bind(id.to_string()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO android_oidc_profile_operations(owner_id,request_id,profile_id,request_mac,receipt_id) VALUES($1,$2,$3,$4,$5)").bind(a.identity_id.to_string()).bind(q.request_id.to_string()).bind(profile.id.to_string()).bind(fingerprint).bind(receipt.to_string()).execute(&mut *tx).await?;
        self.audit_event(
            &mut tx,
            profile.organization_id,
            a.identity_id,
            "oidc_identity_unlinked",
            id,
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }
}
