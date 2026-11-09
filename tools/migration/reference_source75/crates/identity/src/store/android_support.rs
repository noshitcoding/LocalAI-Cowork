//! Support reads use real memberships/ACLs and exact, expiring grant provenance.
use super::*;
pub(super) async fn clear_source(
    tx: &mut Transaction<'_, Any>,
    project: Uuid,
    member: Uuid,
) -> Result<()> {
    sqlx::query("UPDATE android_support_grants SET active=0,revision=revision+1 WHERE id IN(SELECT support_id FROM android_tenant_support_sources WHERE resource_id=$1 AND membership_id=$2) AND active=1").bind(project.to_string()).bind(member.to_string()).execute(&mut **tx).await?;
    sqlx::query(
        "DELETE FROM android_tenant_support_sources WHERE resource_id=$1 AND membership_id=$2",
    )
    .bind(project.to_string())
    .bind(member.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(())
}
impl IdentityStore {
    pub(super) async fn android_validate_support_source(
        &self,
        tx: &mut Transaction<'_, Any>,
        org: Uuid,
        project: Uuid,
        member: Uuid,
        revision: i64,
        action: ContentAction,
    ) -> Result<()> {
        let source=sqlx::query("SELECT s.* FROM android_tenant_support_sources x JOIN android_support_grants s ON s.id=x.support_id WHERE x.resource_id=$1 AND x.membership_id=$2").bind(project.to_string()).bind(member.to_string()).fetch_optional(&mut **tx).await?;
        if let Some(s) = source {
            let now = self.sample(tx).await?;
            let issuer = s.try_get::<String, _>("issuer_id")?;
            let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM identity_subjects i JOIN identity_memberships m ON m.identity_id=i.id JOIN identity_organizations o ON o.id=m.organization_id WHERE i.id=$1 AND i.active=1 AND m.organization_id=$2 AND m.active=1 AND m.role IN('owner','admin') AND o.state='active'").bind(issuer).bind(org.to_string()).fetch_one(&mut **tx).await?;
            if action != ContentAction::Read
                || valid != 1
                || s.try_get::<i64, _>("active")? != 1
                || s.try_get::<i64, _>("expires_at")? <= now
                || s.try_get::<String, _>("organization_id")? != org.to_string()
                || s.try_get::<String, _>("project_id")? != project.to_string()
                || s.try_get::<String, _>("membership_id")? != member.to_string()
                || s.try_get::<i64, _>("grant_revision")? != revision
            {
                return Err(IdentityError::Forbidden);
            }
        }
        Ok(())
    }
    pub async fn android_create_tenant_support_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        org: Uuid,
        project: Uuid,
        user: Uuid,
        expected: i64,
        expires: i64,
        reason: &str,
        support: Uuid,
    ) -> Result<(Uuid, i64)> {
        let now = self.sample(tx).await?;
        if expires <= now
            || expires > now + 7 * 24 * 60 * 60 * 1000
            || reason.trim().is_empty()
            || reason.len() > 2048
            || expected < 0
        {
            return Err(IdentityError::Validation);
        }
        let (_, issuer) = self.org_member(tx, actor, org).await?;
        if !issuer.role.manages_members() {
            return Err(IdentityError::Forbidden);
        }
        self.authorize_in(tx, actor, org, project, ContentAction::Share)
            .await?;
        let subject = sqlx::query("SELECT * FROM identity_subjects WHERE id=$1 AND active=1")
            .bind(user.to_string())
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(IdentityError::NotFound)?;
        let _ = identity(&subject)?;
        let mut original_membership_revision = None;
        let target = match sqlx::query(
            "SELECT * FROM identity_memberships WHERE organization_id=$1 AND identity_id=$2",
        )
        .bind(org.to_string())
        .bind(user.to_string())
        .fetch_optional(&mut **tx)
        .await?
        {
            Some(r) => {
                let m = membership(&r)?;
                if !m.active {
                    return Err(IdentityError::Forbidden);
                }
                m
            }
            None => {
                let member = self
                    .insert_membership(tx, org, user, OrganizationRole::Viewer)
                    .await?;
                original_membership_revision = Some(member.revision);
                member
            }
        };
        let prior=sqlx::query("SELECT revision,active FROM identity_content_grants WHERE resource_id=$1 AND membership_id=$2").bind(project.to_string()).bind(target.id.to_string()).fetch_optional(&mut **tx).await?;
        let revision = prior
            .as_ref()
            .map(|r| r.try_get::<i64, _>("revision"))
            .transpose()?
            .unwrap_or(0);
        if revision != expected {
            return Err(IdentityError::Conflict);
        }
        let managed:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_tenant_support_sources WHERE resource_id=$1 AND membership_id=$2").bind(project.to_string()).bind(target.id.to_string()).fetch_one(&mut **tx).await?;
        if prior
            .as_ref()
            .is_some_and(|r| r.try_get::<i64, _>("active").ok() == Some(1))
            && managed == 0
        {
            return Err(IdentityError::Conflict);
        }
        self.authorize_in(tx, actor, org, project, ContentAction::Share)
            .await?;
        clear_source(tx, project, target.id).await?;
        let next = revision.checked_add(1).ok_or(IdentityError::Corrupt)?;
        sqlx::query("INSERT INTO identity_content_grants(resource_id,membership_id,permissions,active,revision) VALUES($1,$2,1,1,$3) ON CONFLICT(resource_id,membership_id) DO UPDATE SET permissions=1,active=1,revision=$3 WHERE identity_content_grants.revision=$4").bind(project.to_string()).bind(target.id.to_string()).bind(next).bind(revision).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO android_support_grants(id,issuer_id,user_id,project_id,organization_id,membership_id,grant_revision,expires_at,active,revision,reason,created_at,original_membership_revision) VALUES($1,$2,$3,$4,$5,$6,$7,$8,1,1,$9,$10,$11)").bind(support.to_string()).bind(actor.identity_id.to_string()).bind(user.to_string()).bind(project.to_string()).bind(org.to_string()).bind(target.id.to_string()).bind(next).bind(expires).bind(reason).bind(now).bind(original_membership_revision).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO android_tenant_support_sources(resource_id,membership_id,support_id) VALUES($1,$2,$3)").bind(project.to_string()).bind(target.id.to_string()).bind(support.to_string()).execute(&mut **tx).await?;
        self.bump_epoch(tx, org).await?;
        self.audit_event(
            tx,
            Some(org),
            actor.identity_id,
            "support_read_granted",
            project,
        )
        .await?;
        self.org_member(tx, actor, org).await?;
        Ok((target.id, next))
    }
    pub async fn android_revoke_tenant_support_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        support: Uuid,
        expected: i64,
    ) -> Result<()> {
        let s = sqlx::query(
            "SELECT * FROM android_support_grants WHERE id=$1 AND organization_id IS NOT NULL",
        )
        .bind(support.to_string())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(IdentityError::NotFound)?;
        let org = Uuid::parse_str(&s.try_get::<String, _>("organization_id")?)
            .map_err(|_| IdentityError::Corrupt)?;
        let p = Uuid::parse_str(&s.try_get::<String, _>("project_id")?)
            .map_err(|_| IdentityError::Corrupt)?;
        let member = Uuid::parse_str(&s.try_get::<String, _>("membership_id")?)
            .map_err(|_| IdentityError::Corrupt)?;
        let (_, issuer) = self.org_member(tx, actor, org).await?;
        if !issuer.role.manages_members() {
            return Err(IdentityError::Forbidden);
        }
        self.authorize_in(tx, actor, org, p, ContentAction::Share)
            .await?;
        if s.try_get::<i64, _>("revision")? != expected {
            return Err(IdentityError::Conflict);
        }
        // Only this original source may be removed. An independently replaced current ACL survives.
        let owns:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_tenant_support_sources WHERE resource_id=$1 AND membership_id=$2 AND support_id=$3").bind(p.to_string()).bind(member.to_string()).bind(support.to_string()).fetch_one(&mut **tx).await?;
        if owns == 1 {
            sqlx::query("UPDATE identity_content_grants SET active=0,revision=revision+1 WHERE resource_id=$1 AND membership_id=$2 AND revision=$3 AND permissions=1").bind(p.to_string()).bind(member.to_string()).bind(s.try_get::<i64,_>("grant_revision")?).execute(&mut **tx).await?;
            clear_source(tx, p, member).await?;
        } else {
            sqlx::query("UPDATE android_support_grants SET active=0,revision=revision+1 WHERE id=$1 AND revision=$2").bind(support.to_string()).bind(expected).execute(&mut **tx).await?;
        }
        self.bump_epoch(tx, org).await?;
        self.audit_event(tx, Some(org), actor.identity_id, "support_read_revoked", p)
            .await?;
        self.org_member(tx, actor, org).await?;
        Ok(())
    }
}

impl IdentityStore {
    pub(super) async fn android_validate_support_membership(
        &self,
        tx: &mut Transaction<'_, Any>,
        member: &Membership,
    ) -> Result<()> {
        if !self.android_auth_security {
            return Ok(());
        }
        let created:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_support_grants WHERE membership_id=$1 AND organization_id=$2 AND original_membership_revision=$3").bind(member.id.to_string()).bind(member.organization_id.to_string()).bind(member.revision).fetch_one(&mut **tx).await?;
        if created == 0 {
            return Ok(());
        }
        let permanent:i64=sqlx::query_scalar("SELECT COUNT(*) FROM identity_content_grants g JOIN identity_resources p ON p.id=g.resource_id WHERE g.membership_id=$1 AND p.organization_id=$2 AND g.active=1 AND (g.permissions&1)=1 AND NOT EXISTS(SELECT 1 FROM android_tenant_support_sources s WHERE s.resource_id=g.resource_id AND s.membership_id=g.membership_id)").bind(member.id.to_string()).bind(member.organization_id.to_string()).fetch_one(&mut **tx).await?;
        if permanent > 0 {
            return Ok(());
        }
        let current = self.sample(tx).await?;
        let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_tenant_support_sources x JOIN android_support_grants s ON s.id=x.support_id JOIN identity_content_grants g ON g.resource_id=x.resource_id AND g.membership_id=x.membership_id JOIN identity_memberships i ON i.identity_id=s.issuer_id AND i.organization_id=s.organization_id JOIN identity_subjects u ON u.id=i.identity_id WHERE x.membership_id=$1 AND s.organization_id=$2 AND s.active=1 AND s.expires_at>$3 AND g.active=1 AND g.permissions=1 AND g.revision=s.grant_revision AND i.active=1 AND i.role IN('owner','admin') AND u.active=1").bind(member.id.to_string()).bind(member.organization_id.to_string()).bind(current).fetch_one(&mut **tx).await?;
        if active == 0 {
            return Err(IdentityError::Forbidden);
        }
        Ok(())
    }
}

impl IdentityStore {
    pub(super) async fn android_validate_support_member_for(
        &self,
        tx: &mut Transaction<'_, Any>,
        org: Uuid,
        user: Uuid,
    ) -> Result<()> {
        if !self.android_auth_security {
            return Ok(());
        }
        let row=sqlx::query("SELECT * FROM identity_memberships WHERE organization_id=$1 AND identity_id=$2 AND active=1").bind(org.to_string()).bind(user.to_string()).fetch_optional(&mut **tx).await?.ok_or(IdentityError::Forbidden)?;
        self.android_validate_support_membership(tx, &membership(&row)?)
            .await
    }
}
