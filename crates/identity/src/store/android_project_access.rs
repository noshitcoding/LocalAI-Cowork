//! V1 project roles over current Identity grants, with an atomic original reply.
use super::*;
use cowork_contracts::{AndroidContentAccess, AndroidProjectMemberPage, AndroidProjectMemberView};
fn actions(p: Permissions) -> Vec<String> {
    [
        (ContentAction::Read, "read"),
        (ContentAction::Write, "write"),
        (ContentAction::Execute, "execute"),
        (ContentAction::Share, "share"),
    ]
    .into_iter()
    .filter(|(a, _)| p.permits(*a))
    .map(|(_, s)| s.into())
    .collect()
}
fn legacy_role(p: Permissions) -> String {
    match p.bits() {
        1 => "viewer",
        5 => "runner",
        15 => "editor",
        _ => "custom",
    }
    .into()
}
impl IdentityStore {
    pub async fn android_content_access(
        &self,
        actor: &Principal,
        target: Option<(Uuid, Uuid)>,
    ) -> Result<AndroidContentAccess> {
        let mut tx = self.begin().await?;
        self.actor(&mut tx, actor).await?;
        self.require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await?;
        let scope = self.session_scope(&mut tx, actor).await?;
        let ceiling = self.session_scope_permissions(&mut tx, actor).await?;
        let mut organisation_id = scope;
        let mut permissions = Vec::new();
        let mut project_id = None;
        let mut support_access = false;
        if let Some((org, project)) = target {
            let permit = self
                .authorize_content_in_transaction(&mut tx, actor, org, project, ContentAction::Read)
                .await?;
            let (_, member) = self.org_member(&mut tx, actor, org).await?;
            let bits:i64=sqlx::query_scalar("SELECT permissions FROM identity_content_grants WHERE resource_id=$1 AND membership_id=$2 AND active=1").bind(project.to_string()).bind(member.id.to_string()).fetch_one(&mut *tx).await?;
            let effective = bits
                & ceiling
                & if member.role == OrganizationRole::Viewer {
                    1
                } else {
                    15
                };
            let p: Permissions = decoded(&effective.to_string())?;
            if !p.valid() {
                return Err(IdentityError::Corrupt);
            }
            permissions = actions(p);
            self.validate_content_in_transaction(&mut tx, &permit)
                .await?;
            organisation_id = Some(org);
            project_id = Some(project);
            if self.android_auth_security {
                support_access = sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM android_tenant_support_sources WHERE resource_id=$1 AND membership_id=$2").bind(project.to_string()).bind(member.id.to_string()).fetch_one(&mut *tx).await? == 1;
            }
        }
        let value = AndroidContentAccess {
            schema_version: 2,
            actor_user_id: actor.identity_id,
            organisation_id,
            project_id,
            managed: organisation_id.is_some(),
            instance_viewer: ceiling == 1,
            local_membership_allowed: true,
            automatic_content_access: false,
            support_access,
            permissions,
        };
        tx.commit().await?;
        Ok(value)
    }
    pub async fn android_project_members(
        &self,
        actor: &Principal,
        org: Uuid,
        project: Uuid,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<AndroidProjectMemberPage> {
        if !(1..=100).contains(&limit) {
            return Err(IdentityError::Validation);
        }
        let mut tx = self.begin().await?;
        let permit = self
            .authorize_content_in_transaction(&mut tx, actor, org, project, ContentAction::Share)
            .await?;
        let rows=sqlx::query("SELECT m.id AS membership_id,COALESCE(g.permissions,1) AS permissions,COALESCE(g.active,0) AS active,COALESCE(g.revision,0) AS revision,m.identity_id,m.role AS organization_role,m.revision AS membership_revision,s.display_name FROM identity_memberships m JOIN identity_subjects s ON s.id=m.identity_id LEFT JOIN identity_content_grants g ON g.membership_id=m.id AND g.resource_id=$1 WHERE m.organization_id=$2 AND m.active=1 AND s.active=1 AND m.id>$3 ORDER BY m.id LIMIT $4").bind(project.to_string()).bind(org.to_string()).bind(after.map(|v|v.to_string()).unwrap_or_default()).bind(i64::from(limit)+1).fetch_all(&mut *tx).await?;
        let more = rows.len() > limit as usize;
        let mut items = Vec::new();
        for row in rows.into_iter().take(limit as usize) {
            let bits = row.try_get::<i64, _>("permissions")?
                & if row.try_get::<String, _>("organization_role")? == "viewer" {
                    1
                } else {
                    15
                };
            let p: Permissions = decoded(&bits.to_string())?;
            if !p.valid() {
                return Err(IdentityError::Corrupt);
            }
            let mut active = row.try_get::<i64, _>("active")? == 1;
            if active && self.android_auth_security {
                match self
                    .android_validate_support_source(
                        &mut tx,
                        org,
                        project,
                        identifier(row.try_get("membership_id")?)?,
                        row.try_get("revision")?,
                        ContentAction::Read,
                    )
                    .await
                {
                    Ok(()) => {}
                    Err(IdentityError::Forbidden) => active = false,
                    Err(e) => return Err(e),
                }
            }
            items.push(AndroidProjectMemberView {
                schema_version: 2,
                project_id: project,
                user_id: identifier(row.try_get("identity_id")?)?,
                membership_id: identifier(row.try_get("membership_id")?)?,
                display_name: row.try_get("display_name")?,
                role: if active {
                    legacy_role(p)
                } else {
                    "ungranted".into()
                },
                permissions: if active { actions(p) } else { vec![] },
                active,
                revision: row.try_get("revision")?,
                membership_revision: row.try_get("membership_revision")?,
            })
        }
        self.validate_content_in_transaction(&mut tx, &permit)
            .await?;
        let next_cursor = if more {
            items.last().map(|v| v.membership_id)
        } else {
            None
        };
        tx.commit().await?;
        Ok(AndroidProjectMemberPage { items, next_cursor })
    }
    /// A nullable role revokes only this actual content ACL. It neither removes an
    /// organisation member nor grants a provider, tool, runner or workflow right.
    #[expect(
        clippy::too_many_arguments,
        reason = "Keep actor, tenant, project, user, grant CAS and replay key explicit."
    )]
    pub async fn android_set_project_member(
        &self,
        actor: &Principal,
        org: Uuid,
        project: Uuid,
        user: Uuid,
        role: Option<&str>,
        expected: Option<i64>,
        key: Option<&str>,
    ) -> Result<ContentGrant> {
        let permissions = match role {
            Some("viewer") | None => Permissions::read_only(),
            Some("runner") => {
                Permissions::from_actions(&[ContentAction::Read, ContentAction::Execute])
            }
            Some("editor") => Permissions::all(),
            _ => return Err(IdentityError::Validation),
        };
        if user.is_nil()
            || expected.is_some_and(|v| v < 0)
            || key.is_some_and(|k| k.is_empty() || k.len() > 200 || k.chars().any(char::is_control))
        {
            return Err(IdentityError::Validation);
        }
        let request = format!(
            "{:x}",
            Sha256::digest(encoded(&(project, user, role, expected))?.as_bytes())
        );
        let mut tx = self.begin().await?;
        // Original receipt reads need current actual identity/organisation visibility,
        // not the old write grant and never authorization derived from that receipt.
        self.org_member(&mut tx, actor, org).await?;
        if let Some(key) = key {
            let operation = format!("android-project-member:{key}");
            if let Some(old) = self
                .operation(&mut tx, actor.identity_id, org, &operation, &request)
                .await?
            {
                tx.commit().await?;
                return Ok(old);
            }
        }
        let permit = self
            .authorize_content_in_transaction(&mut tx, actor, org, project, ContentAction::Share)
            .await?;
        let row=sqlx::query("SELECT m.* FROM identity_memberships m JOIN identity_subjects s ON s.id=m.identity_id WHERE m.identity_id=$1 AND m.organization_id=$2 AND m.active=1 AND s.active=1").bind(user.to_string()).bind(org.to_string()).fetch_optional(&mut *tx).await?.ok_or(IdentityError::NotFound)?;
        let target = membership(&row)?;
        if target.role == OrganizationRole::Viewer
            && role.is_some()
            && permissions != Permissions::read_only()
        {
            return Err(IdentityError::Forbidden);
        }
        let prior=sqlx::query("SELECT permissions,active,revision FROM identity_content_grants WHERE resource_id=$1 AND membership_id=$2").bind(project.to_string()).bind(target.id.to_string()).fetch_optional(&mut *tx).await?;
        let revision = prior
            .as_ref()
            .map(|r| r.try_get::<i64, _>("revision"))
            .transpose()?
            .unwrap_or(0);
        if expected.is_some_and(|e| e != revision) {
            return Err(IdentityError::Conflict);
        }
        let active = role.is_some();
        let support_managed = if self.android_auth_security {
            sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM android_tenant_support_sources WHERE resource_id=$1 AND membership_id=$2").bind(project.to_string()).bind(target.id.to_string()).fetch_one(&mut *tx).await? == 1
        } else {
            false
        };
        let same = !support_managed
            && prior.as_ref().is_some_and(|r| {
                r.try_get::<i64, _>("active").ok() == Some(i64::from(active))
                    && r.try_get::<i64, _>("permissions").ok() == Some(permissions.bits())
            });
        let value = ContentGrant {
            resource_id: project,
            membership_id: target.id,
            permissions,
            active,
            revision: if same {
                revision
            } else {
                revision.checked_add(1).ok_or(IdentityError::Corrupt)?
            },
        };
        self.validate_content_in_transaction(&mut tx, &permit)
            .await?;
        if !same {
            if self.android_auth_security {
                super::android_support::clear_source(&mut tx, project, target.id).await?;
            }
            if revision==0{sqlx::query("INSERT INTO identity_content_grants(resource_id,membership_id,permissions,active,revision) VALUES($1,$2,$3,$4,1)").bind(project.to_string()).bind(target.id.to_string()).bind(permissions.bits()).bind(i64::from(active)).execute(&mut *tx).await?;}
   else if sqlx::query("UPDATE identity_content_grants SET permissions=$1,active=$2,revision=$3 WHERE resource_id=$4 AND membership_id=$5 AND revision=$6").bind(permissions.bits()).bind(i64::from(active)).bind(value.revision).bind(project.to_string()).bind(target.id.to_string()).bind(revision).execute(&mut *tx).await?.rows_affected()!=1{return Err(IdentityError::Conflict)}
            self.bump_epoch(&mut tx, org).await?;
            self.audit_event(
                &mut tx,
                Some(org),
                actor.identity_id,
                if active {
                    "android_project_access_changed"
                } else {
                    "android_project_access_revoked"
                },
                project,
            )
            .await?;
        }
        if let Some(key) = key {
            self.save_operation(
                &mut tx,
                actor.identity_id,
                org,
                &format!("android-project-member:{key}"),
                &request,
                &value,
            )
            .await?;
        }
        // Revalidate live session/membership after audit/epoch waits; do not compare
        // the deliberately advanced project authorization epoch with the old permit.
        self.org_member(&mut tx, actor, org).await?;
        self.require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await?;
        tx.commit().await?;
        Ok(value)
    }
}
