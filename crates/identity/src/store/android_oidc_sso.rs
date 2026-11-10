//! Real organization SSO policies and membership grants from checked signed groups.
use super::*;
use cowork_contracts::{
    AndroidOidcGroupMapping, AndroidOidcMembershipOrigin, AndroidOidcSsoProvider,
    AndroidOidcSsoSettings, AndroidOidcSsoWrite,
};
use cowork_secrets::McpSecretScope;
#[derive(Clone)]
pub(super) struct Policy {
    pub revision: i64,
    pub epoch: Uuid,
    pub enabled: bool,
    pub profile: Option<Uuid>,
    pub provision: bool,
    pub mappings: Vec<AndroidOidcGroupMapping>,
    pub sha: String,
}
pub(super) async fn read(tx: &mut Transaction<'_, Any>, org: Uuid) -> Result<Policy> {
    let r = sqlx::query("SELECT * FROM android_oidc_org_sso WHERE organization_id=$1")
        .bind(org.to_string())
        .fetch_optional(&mut **tx)
        .await?;
    if let Some(r) = r {
        let profile = r
            .try_get::<Option<String>, _>("profile_id")?
            .map(identifier)
            .transpose()?;
        let mappings: Vec<AndroidOidcGroupMapping> =
            decoded(&r.try_get::<String, _>("group_mappings")?)?;
        let p = Policy {
            revision: r.try_get("revision")?,
            epoch: identifier(r.try_get("epoch")?)?,
            enabled: r.try_get::<i64, _>("enabled")? == 1,
            profile,
            provision: r.try_get::<i64, _>("auto_provision")? == 1,
            mappings,
            sha: r.try_get("config_sha256")?,
        };
        if p.sha != policy_sha(p.enabled, p.profile, p.provision, &p.mappings)? {
            return Err(IdentityError::Corrupt);
        }
        Ok(p)
    } else {
        Ok(Policy {
            revision: 0,
            epoch: Uuid::nil(),
            enabled: false,
            profile: None,
            provision: false,
            mappings: vec![],
            sha: policy_sha(false, None, false, &[])?,
        })
    }
}
fn policy_sha(
    enabled: bool,
    profile: Option<Uuid>,
    provision: bool,
    mappings: &[AndroidOidcGroupMapping],
) -> Result<String> {
    Ok(digest(
        encoded(&(enabled, profile, provision, mappings))?.as_bytes(),
    ))
}
pub(super) async fn admission(
    tx: &mut Transaction<'_, Any>,
    profile: &AndroidOidcProfile,
) -> Result<Option<Policy>> {
    let Some(org) = profile.organization_id else {
        return Ok(None);
    };
    let p = read(tx, org).await?;
    if !p.enabled || p.profile != Some(profile.id) {
        return Err(IdentityError::Forbidden);
    }
    Ok(Some(p))
}
fn matching_role(p: &Policy, groups: &[String]) -> Option<OrganizationRole> {
    if p.mappings.is_empty() {
        return Some(OrganizationRole::Member);
    }
    let rank = |role: &str| match role {
        "admin" => 3,
        "member" => 2,
        "auditor" => 1,
        _ => 0,
    };
    let best = p
        .mappings
        .iter()
        .filter(|m| groups.contains(&m.group))
        .max_by_key(|m| rank(&m.role));
    best.map(|m| match m.role.as_str() {
        "admin" => OrganizationRole::Admin,
        "member" => OrganizationRole::Member,
        _ => OrganizationRole::Viewer,
    })
}
pub(super) async fn revoke_profile(tx: &mut Transaction<'_, Any>, profile: Uuid) -> Result<()> {
    sqlx::query("UPDATE identity_memberships SET active=0,revision=revision+1 WHERE active=1 AND EXISTS(SELECT 1 FROM android_oidc_membership_origins g WHERE g.membership_id=identity_memberships.id AND g.profile_id=$1 AND g.active=1 AND g.membership_revision=identity_memberships.revision)").bind(profile.to_string()).execute(&mut **tx).await?;
    sqlx::query(
        "UPDATE android_oidc_membership_origins SET active=0 WHERE profile_id=$1 AND active=1",
    )
    .bind(profile.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(())
}
pub(super) async fn revoke_external(tx: &mut Transaction<'_, Any>, external: Uuid) -> Result<()> {
    sqlx::query("UPDATE identity_memberships SET active=0,revision=revision+1 WHERE active=1 AND EXISTS(SELECT 1 FROM android_oidc_membership_origins g WHERE g.membership_id=identity_memberships.id AND g.external_identity_id=$1 AND g.active=1 AND g.membership_revision=identity_memberships.revision)").bind(external.to_string()).execute(&mut **tx).await?;
    sqlx::query("UPDATE android_oidc_membership_origins SET active=0 WHERE external_identity_id=$1 AND active=1").bind(external.to_string()).execute(&mut **tx).await?;
    Ok(())
}
async fn settings(tx: &mut Transaction<'_, Any>, org: Uuid) -> Result<AndroidOidcSsoSettings> {
    let p = read(tx, org).await?;
    let rows = sqlx::query(
        "SELECT id FROM android_oidc_profiles WHERE organization_id=$1 ORDER BY id LIMIT 1001",
    )
    .bind(org.to_string())
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() > 1000 {
        return Err(IdentityError::QuotaExceeded);
    }
    let mut providers = vec![];
    let mut available = false;
    for row in rows {
        let v = profiles::read(tx, identifier(row.try_get("id")?)?).await?;
        if v.active {
            if p.profile == Some(v.id) {
                available = true
            }
            providers.push(AndroidOidcSsoProvider {
                id: v.id.to_string(),
                label: v.name,
                issuer: v.config.issuer,
                groups_claim: v.config.groups_claim,
            });
        }
    }
    Ok(AndroidOidcSsoSettings {
        organisation_id: org,
        revision: p.revision,
        enabled: p.enabled,
        available,
        provider_id: p.profile.map(|id| id.to_string()),
        auto_provision: p.provision,
        group_mappings: p.mappings,
        providers,
    })
}
impl IdentityStore {
    pub async fn android_oidc_sso_settings(
        &self,
        a: &Principal,
        org: Uuid,
    ) -> Result<AndroidOidcSsoSettings> {
        let mut tx = self.begin().await?;
        self.manage_oidc(&mut tx, a, Some(org)).await?;
        let value = settings(&mut tx, org).await?;
        self.manage_oidc(&mut tx, a, Some(org)).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn android_oidc_sso_write(
        &self,
        a: &Principal,
        org: Uuid,
        operation: Uuid,
        q: &AndroidOidcSsoWrite,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<AndroidOidcSsoSettings> {
        if operation.is_nil()
            || q.expected_revision < 0
            || q.group_mappings.len() > 100
            || q.group_mappings.iter().any(|m| {
                m.group.is_empty()
                    || m.group.len() > 256
                    || m.group.chars().any(char::is_control)
                    || !matches!(m.role.as_str(), "admin" | "member" | "auditor")
            })
            || q.group_mappings.iter().any(|m| {
                q.group_mappings
                    .iter()
                    .filter(|n| n.group == m.group)
                    .count()
                    != 1
            })
        {
            return Err(IdentityError::Validation);
        }
        let selected = q
            .provider_id
            .as_ref()
            .map(|v| identifier(v.clone()))
            .transpose()?;
        if q.enabled && selected.is_none() {
            return Err(IdentityError::Validation);
        }
        let fingerprint = mac(keys, key, &(org, q))?;
        let mut tx = self.begin().await?;
        self.manage_oidc(&mut tx, a, Some(org)).await?;
        if let Some(r) = sqlx::query(
            "SELECT * FROM android_oidc_sso_operations WHERE owner_id=$1 AND request_id=$2",
        )
        .bind(a.identity_id.to_string())
        .bind(operation.to_string())
        .fetch_optional(&mut *tx)
        .await?
        {
            if r.try_get::<String, _>("request_mac")? != fingerprint
                || r.try_get::<String, _>("organization_id")? != org.to_string()
            {
                return Err(IdentityError::Conflict);
            }
            let value = decoded::<AndroidOidcSsoSettings>(
                &String::from_utf8(
                    cipher::read(
                        &mut tx,
                        keys,
                        identifier(r.try_get("receipt_id")?)?,
                        org,
                        "oauth_response",
                    )
                    .await?
                    .to_vec(),
                )
                .map_err(|_| IdentityError::Corrupt)?,
            )?;
            self.manage_oidc(&mut tx, a, Some(org)).await?;
            tx.commit().await?;
            return Ok(value);
        }
        let old = read(&mut tx, org).await?;
        if old.revision != q.expected_revision {
            return Err(IdentityError::Conflict);
        }
        let (_, actor_member) = self.org_member(&mut tx, a, org).await?;
        if q.group_mappings.iter().any(|m| m.role == "admin")
            && actor_member.role != OrganizationRole::Owner
        {
            return Err(IdentityError::Forbidden);
        }
        if let Some(id) = selected {
            let p = profiles::read(&mut tx, id).await?;
            if p.organization_id != Some(org) || q.enabled && !p.active {
                return Err(IdentityError::Forbidden);
            }
        }
        let revision = old
            .revision
            .checked_add(1)
            .ok_or(IdentityError::Validation)?;
        let epoch = Uuid::new_v4();
        let sha = policy_sha(q.enabled, selected, q.auto_provision, &q.group_mappings)?;
        let mut value = settings(&mut tx, org).await?;
        value.revision = revision;
        value.enabled = q.enabled;
        value.provider_id = q.provider_id.clone();
        value.auto_provision = q.auto_provision;
        value.group_mappings = q.group_mappings.clone();
        value.available =
            selected.is_some_and(|id| value.providers.iter().any(|p| p.id == id.to_string()));
        let record = Uuid::new_v4();
        let at = self.sample(&mut tx).await?;
        let scope = McpSecretScope {
            owner_kind: "team".into(),
            owner_id: org.to_string(),
            project_id: org.to_string(),
            server_id: org.to_string(),
            record_id: record.to_string(),
            revision,
            epoch,
            configuration_sha256: sha.clone(),
            purpose: "oauth_response".into(),
        };
        let receipt = cipher::put_scope(
            &mut tx,
            keys,
            key,
            scope,
            &serde_json::to_vec(&value).map_err(|_| IdentityError::Corrupt)?,
            at,
        )
        .await?;
        self.manage_oidc(&mut tx, a, Some(org)).await?;
        if let Some(id) = selected {
            let profile = profiles::read(&mut tx, id).await?;
            if profile.organization_id != Some(org) || q.enabled && !profile.active {
                return Err(IdentityError::Forbidden);
            }
        }
        if old.revision==0{sqlx::query("INSERT INTO android_oidc_org_sso(organization_id,revision,epoch,enabled,profile_id,auto_provision,group_mappings,config_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(org.to_string()).bind(revision).bind(epoch.to_string()).bind(i64::from(q.enabled)).bind(selected.map(|id|id.to_string())).bind(i64::from(q.auto_provision)).bind(encoded(&q.group_mappings)?).bind(&sha).execute(&mut *tx).await?;}else if sqlx::query("UPDATE android_oidc_org_sso SET revision=$1,epoch=$2,enabled=$3,profile_id=$4,auto_provision=$5,group_mappings=$6,config_sha256=$7 WHERE organization_id=$8 AND revision=$9 AND epoch=$10").bind(revision).bind(epoch.to_string()).bind(i64::from(q.enabled)).bind(selected.map(|id|id.to_string())).bind(i64::from(q.auto_provision)).bind(encoded(&q.group_mappings)?).bind(&sha).bind(org.to_string()).bind(old.revision).bind(old.epoch.to_string()).execute(&mut *tx).await?.rows_affected()!=1{return Err(IdentityError::Conflict)}
        sqlx::query("UPDATE identity_memberships SET active=0,revision=revision+1 WHERE organization_id=$1 AND active=1 AND EXISTS(SELECT 1 FROM android_oidc_membership_origins g WHERE g.membership_id=identity_memberships.id AND g.membership_revision=identity_memberships.revision AND g.active=1)").bind(org.to_string()).execute(&mut *tx).await?;
        sqlx::query("UPDATE android_oidc_membership_origins SET active=0 WHERE organization_id=$1 AND active=1").bind(org.to_string()).execute(&mut *tx).await?;
        sqlx::query("UPDATE identity_sessions SET revoked=1,revision=revision+1 WHERE revoked=0 AND id IN(SELECT o.session_id FROM android_oidc_session_origins o JOIN android_oidc_profiles p ON p.id=o.profile_id WHERE p.organization_id=$1)").bind(org.to_string()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO android_oidc_sso_operations(owner_id,request_id,organization_id,request_mac,receipt_id) VALUES($1,$2,$3,$4,$5)").bind(a.identity_id.to_string()).bind(operation.to_string()).bind(org.to_string()).bind(fingerprint).bind(receipt.to_string()).execute(&mut *tx).await?;
        self.audit_event(
            &mut tx,
            Some(org),
            a.identity_id,
            "oidc_sso_policy_changed",
            org,
        )
        .await?;
        tx.commit().await?;
        Ok(value)
    }
    pub(in crate::store) async fn validate_oidc_membership_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        member: &Membership,
    ) -> Result<()> {
        if !self.android_auth_security {
            return Ok(());
        }
        let Some(g) =
            sqlx::query("SELECT * FROM android_oidc_membership_origins WHERE membership_id=$1")
                .bind(member.id.to_string())
                .fetch_optional(&mut **tx)
                .await?
        else {
            return Ok(());
        };
        if g.try_get::<i64, _>("membership_revision")? != member.revision {
            return Ok(());
        } // A later explicit local mutation owns its new revision.
        let profile = profiles::read(tx, identifier(g.try_get("profile_id")?)?).await?;
        let p = read(tx, member.organization_id).await?;
        let at = self.sample(tx).await?;
        if !member.active
            || g.try_get::<i64, _>("active")? != 1
            || g.try_get::<String, _>("identity_id")? != member.identity_id.to_string()
            || g.try_get::<String, _>("organization_id")? != member.organization_id.to_string()
            || g.try_get::<String, _>("role")? != word(&member.role)?
            || !profile.active
            || profile.organization_id != Some(member.organization_id)
            || profile.revision != g.try_get::<i64, _>("profile_revision")?
            || !p.enabled
            || p.profile != Some(profile.id)
            || p.revision != g.try_get::<i64, _>("settings_revision")?
            || p.epoch.to_string() != g.try_get::<String, _>("settings_epoch")?
            || g.try_get::<i64, _>("expires_at")? <= at
        {
            return Err(IdentityError::Forbidden);
        }
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_oidc_external_identities e JOIN android_oidc_flows f ON f.id=$1 WHERE e.id=$2 AND e.profile_id=$3 AND e.identity_id=$4 AND e.revision=$5 AND e.active=1 AND f.phase='committed'").bind(g.try_get::<String,_>("flow_id")?).bind(g.try_get::<String,_>("external_identity_id")?).bind(profile.id.to_string()).bind(member.identity_id.to_string()).bind(g.try_get::<i64,_>("external_revision")?).fetch_one(&mut **tx).await?;
        if count != 1 {
            return Err(IdentityError::Forbidden);
        }
        Ok(())
    }
    pub(in crate::store) async fn validate_oidc_member_for(
        &self,
        tx: &mut Transaction<'_, Any>,
        org: Uuid,
        owner: Uuid,
    ) -> Result<()> {
        let row=sqlx::query("SELECT * FROM identity_memberships WHERE organization_id=$1 AND identity_id=$2 AND active=1").bind(org.to_string()).bind(owner.to_string()).fetch_optional(&mut **tx).await?.ok_or(IdentityError::Forbidden)?;
        self.validate_oidc_membership_in_transaction(tx, &membership(&row)?)
            .await
    }
    pub async fn android_oidc_membership_origins(
        &self,
        a: &Principal,
        org: Uuid,
    ) -> Result<Vec<AndroidOidcMembershipOrigin>> {
        let mut tx = self.begin().await?;
        self.manage_oidc(&mut tx, a, Some(org)).await?;
        let rows=sqlx::query("SELECT * FROM android_oidc_membership_origins WHERE organization_id=$1 ORDER BY membership_id LIMIT 1001").bind(org.to_string()).fetch_all(&mut *tx).await?;
        if rows.len() > 1000 {
            return Err(IdentityError::QuotaExceeded);
        }
        let mut values = vec![];
        for r in rows {
            values.push(AndroidOidcMembershipOrigin {
                membership_id: identifier(r.try_get("membership_id")?)?,
                organisation_id: org,
                identity_id: identifier(r.try_get("identity_id")?)?,
                profile_id: identifier(r.try_get("profile_id")?)?,
                external_identity_id: identifier(r.try_get("external_identity_id")?)?,
                flow_id: identifier(r.try_get("flow_id")?)?,
                claims_sha256: r.try_get("claims_sha256")?,
                groups_sha256: r.try_get("groups_sha256")?,
                settings_revision: r.try_get("settings_revision")?,
                membership_revision: r.try_get("membership_revision")?,
                role: r.try_get("role")?,
                active: r.try_get::<i64, _>("active")? == 1,
                expires_at: r.try_get("expires_at")?,
            });
        }
        self.manage_oidc(&mut tx, a, Some(org)).await?;
        tx.commit().await?;
        Ok(values)
    }
}
#[expect(
    clippy::too_many_arguments,
    reason = "Keep the transaction, SSO policy and original external identity bindings explicit."
)]
pub(super) async fn apply(
    store: &IdentityStore,
    tx: &mut Transaction<'_, Any>,
    profile: &AndroidOidcProfile,
    f: &sqlx::any::AnyRow,
    owner: Uuid,
    external: Uuid,
    external_revision: i64,
    subject: &super::super::android_oidc_verifier::VerifiedOidcSubject,
) -> Result<bool> {
    let Some(org) = profile.organization_id else {
        return Ok(true);
    };
    let p = admission(tx, profile)
        .await?
        .ok_or(IdentityError::Corrupt)?;
    let existing=sqlx::query("SELECT * FROM identity_memberships WHERE organization_id=$1 AND identity_id=$2 AND active=1").bind(org.to_string()).bind(owner.to_string()).fetch_optional(&mut **tx).await?;
    if let Some(row) = &existing {
        let member = membership(row)?;
        let source=sqlx::query("SELECT membership_revision FROM android_oidc_membership_origins WHERE membership_id=$1").bind(member.id.to_string()).fetch_optional(&mut **tx).await?;
        if source.is_none_or(|s| {
            s.try_get::<i64, _>("membership_revision").ok() != Some(member.revision)
        }) {
            return Ok(true);
        }
    }
    let role = matching_role(&p, &subject.groups);
    let inactive_local:i64=sqlx::query_scalar("SELECT COUNT(*) FROM identity_memberships m WHERE m.organization_id=$1 AND m.identity_id=$2 AND m.active=0 AND NOT EXISTS(SELECT 1 FROM android_oidc_membership_origins g WHERE g.membership_id=m.id)").bind(org.to_string()).bind(owner.to_string()).fetch_one(&mut **tx).await?;
    if role.is_none() || existing.is_none() && (!p.provision || inactive_local > 0) {
        if let Some(row) = existing {
            let member = membership(&row)?;
            sqlx::query("UPDATE identity_memberships SET active=0,revision=revision+1 WHERE id=$1 AND revision=$2 AND active=1").bind(member.id.to_string()).bind(member.revision).execute(&mut **tx).await?;
            sqlx::query(
                "UPDATE android_oidc_membership_origins SET active=0 WHERE membership_id=$1",
            )
            .bind(member.id.to_string())
            .execute(&mut **tx)
            .await?;
            sqlx::query("UPDATE identity_sessions SET revoked=1,revision=revision+1 WHERE identity_id=$1 AND revoked=0 AND id IN(SELECT session_id FROM android_oidc_session_origins WHERE profile_id=$2)").bind(owner.to_string()).bind(profile.id.to_string()).execute(&mut **tx).await?;
        }
        return Ok(false);
    }
    let role = role.ok_or(IdentityError::Corrupt)?;
    let member = if let Some(row) = existing {
        let mut member = membership(&row)?;
        if member.role != role {
            sqlx::query("UPDATE identity_sessions SET revoked=1,revision=revision+1 WHERE identity_id=$1 AND revoked=0 AND id IN(SELECT session_id FROM android_oidc_session_origins WHERE profile_id=$2)").bind(owner.to_string()).bind(profile.id.to_string()).execute(&mut **tx).await?;
        }
        member.revision = member
            .revision
            .checked_add(1)
            .ok_or(IdentityError::Corrupt)?;
        member.role = role;
        if sqlx::query("UPDATE identity_memberships SET role=$1,revision=$2 WHERE id=$3 AND revision=$4 AND active=1").bind(word(&role)?).bind(member.revision).bind(member.id.to_string()).bind(member.revision-1).execute(&mut **tx).await?.rows_affected()!=1{return Err(IdentityError::Conflict)}
        member
    } else {
        store.insert_membership(tx, org, owner, role).await?
    };
    sqlx::query("INSERT INTO android_oidc_membership_origins(membership_id,organization_id,identity_id,profile_id,external_identity_id,profile_revision,external_revision,settings_revision,settings_epoch,flow_id,claims_sha256,groups_sha256,role,membership_revision,expires_at,active) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,1) ON CONFLICT(membership_id) DO UPDATE SET profile_id=excluded.profile_id,external_identity_id=excluded.external_identity_id,profile_revision=excluded.profile_revision,external_revision=excluded.external_revision,settings_revision=excluded.settings_revision,settings_epoch=excluded.settings_epoch,flow_id=excluded.flow_id,claims_sha256=excluded.claims_sha256,groups_sha256=excluded.groups_sha256,role=excluded.role,membership_revision=excluded.membership_revision,expires_at=excluded.expires_at,active=1").bind(member.id.to_string()).bind(org.to_string()).bind(owner.to_string()).bind(profile.id.to_string()).bind(external.to_string()).bind(profile.revision).bind(external_revision).bind(p.revision).bind(p.epoch.to_string()).bind(f.try_get::<String,_>("id")?).bind(&subject.claims_sha256).bind(digest(encoded(&subject.groups)?.as_bytes())).bind(word(&role)?).bind(member.revision).bind(subject.expires_at).execute(&mut **tx).await?;
    Ok(true)
}
