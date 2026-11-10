//! Explicit present-day administrative mappings, never legacy principal adoption.
use super::*;
use std::collections::BTreeSet;
use zeroize::Zeroizing;

pub enum MigrationIdentityTarget {
    Existing { identity_id: Uuid, expected_revision: i64 },
    New { username: String, display_name: String, password: Zeroizing<String> },
}

pub struct MigrationProjectGrant {
    pub project_id: Uuid,
    pub expected_resource_revision: i64,
    pub expected_grant_revision: i64,
    pub write: bool,
    pub share: bool,
}

pub struct MigrationIdentityMapping {
    pub source_user_id: String,
    pub target: MigrationIdentityTarget,
    pub role: OrganizationRole,
    pub expected_membership_revision: i64,
    pub project_grants: Vec<MigrationProjectGrant>,
}

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationIdentityBinding {
    pub source_user_id: String,
    pub identity: Identity,
    pub membership: Membership,
    pub grants: Vec<ContentGrant>,
}

impl LockedIdentityQuota<'_, '_, '_> {
    /// The import must persist its exact mapping receipt in the caller transaction.
    /// A caller retry reads that receipt before requesting any new subject IDs.
    pub async fn apply_migration_identities_in_transaction(
        &mut self,
        actor: &Principal,
        org: Uuid,
        mappings: &[MigrationIdentityMapping],
    ) -> Result<Vec<MigrationIdentityBinding>> {
        self.usable()?;
        self.poisoned = true;
        let mut operation = self.tx.begin().await?;
        let result = self.store.apply_migration_identities_locked(&mut operation, actor, org, mappings).await;
        finish_quota_operation(operation, result, &mut self.poisoned).await
    }
}

impl IdentityStore {
    async fn apply_migration_identities_locked(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        org: Uuid,
        mappings: &[MigrationIdentityMapping],
    ) -> Result<Vec<MigrationIdentityBinding>> {
        if mappings.len() > 1000 || mappings.iter().map(|m| m.project_grants.len()).sum::<usize>() > 4000 {
            return Err(IdentityError::Validation);
        }
        let (_, current) = self.org_member(tx, actor, org).await?;
        if !current.role.manages_members() {
            return Err(IdentityError::Forbidden);
        }
        let mut source_ids = BTreeSet::new();
        let mut targets = BTreeSet::new();
        let mut result = Vec::with_capacity(mappings.len());
        for mapping in mappings {
            if mapping.source_user_id.is_empty() || mapping.source_user_id.len() > 256
                || mapping.source_user_id.chars().any(char::is_control)
                || !source_ids.insert(mapping.source_user_id.clone())
                || mapping.expected_membership_revision < 0
                || !Self::may_manage(current.role, mapping.role)
            {
                return Err(IdentityError::Forbidden);
            }
            // Revalidate the current administrator after every earlier mapping.
            let (_, administrator) = self.org_member(tx, actor, org).await?;
            if !Self::may_manage(administrator.role, mapping.role) {
                return Err(IdentityError::Forbidden);
            }
            let subject = match &mapping.target {
                MigrationIdentityTarget::Existing { identity_id, expected_revision } => {
                    let row = sqlx::query("SELECT * FROM identity_subjects WHERE id = $1")
                        .bind(identity_id.to_string()).fetch_optional(&mut **tx).await?
                        .ok_or(IdentityError::NotFound)?;
                    let subject = identity(&row)?;
                    if !subject.active || subject.revision != *expected_revision {
                        return Err(IdentityError::Conflict);
                    }
                    subject
                }
                MigrationIdentityTarget::New { username, display_name, password } => {
                    if mapping.expected_membership_revision != 0 {
                        return Err(IdentityError::Conflict);
                    }
                    self.insert_identity(tx, username, display_name, password.as_str(), false).await?
                }
            };
            if !targets.insert(subject.id) {
                return Err(IdentityError::Validation);
            }
            let existing = sqlx::query("SELECT * FROM identity_memberships WHERE organization_id = $1 AND identity_id = $2 AND active = 1")
                .bind(org.to_string()).bind(subject.id.to_string()).fetch_optional(&mut **tx).await?;
            let member = if let Some(row) = existing {
                let mut prior = membership(&row)?;
                if prior.revision != mapping.expected_membership_revision
                    || !Self::may_manage(administrator.role, prior.role)
                {
                    return Err(IdentityError::Conflict);
                }
                if prior.role != mapping.role {
                    if prior.role == OrganizationRole::Owner {
                        let count: i64 = sqlx::query("SELECT COUNT(*) AS count FROM identity_memberships m JOIN identity_subjects s ON s.id = m.identity_id WHERE m.organization_id = $1 AND m.role = 'owner' AND m.active = 1 AND s.active = 1")
                            .bind(org.to_string()).fetch_one(&mut **tx).await?.try_get("count")?;
                        if count <= 1 { return Err(IdentityError::Forbidden); }
                    }
                    prior.role = mapping.role;
                    prior.revision += 1;
                    sqlx::query("UPDATE identity_memberships SET role = $1,revision = $2 WHERE id = $3")
                        .bind(word(&prior.role)?).bind(prior.revision).bind(prior.id.to_string())
                        .execute(&mut **tx).await?;
                }
                prior
            } else {
                if mapping.expected_membership_revision != 0 { return Err(IdentityError::Conflict); }
                self.insert_membership(tx, org, subject.id, mapping.role).await?
            };
            let mut projects = BTreeSet::new();
            let mut grants = Vec::with_capacity(mapping.project_grants.len());
            for request in &mapping.project_grants {
                if !projects.insert(request.project_id) || request.expected_resource_revision < 1
                    || request.expected_grant_revision < 0
                    || (member.role == OrganizationRole::Viewer && (request.write || request.share))
                { return Err(IdentityError::Validation); }
                self.authorize_in(tx, actor, org, request.project_id, ContentAction::Share).await?;
                let row = sqlx::query("SELECT organization_id,kind,revision FROM identity_resources WHERE id = $1")
                    .bind(request.project_id.to_string()).fetch_optional(&mut **tx).await?
                    .ok_or(IdentityError::NotFound)?;
                if row.try_get::<String,_>("organization_id")? != org.to_string()
                    || row.try_get::<String,_>("kind")? != "project"
                    || row.try_get::<i64,_>("revision")? != request.expected_resource_revision
                { return Err(IdentityError::Conflict); }
                let prior = sqlx::query("SELECT revision FROM identity_content_grants WHERE resource_id = $1 AND membership_id = $2")
                    .bind(request.project_id.to_string()).bind(member.id.to_string())
                    .fetch_optional(&mut **tx).await?;
                let revision = prior.map(|row| row.try_get::<i64,_>("revision")).transpose()?.unwrap_or(0);
                if revision != request.expected_grant_revision { return Err(IdentityError::Conflict); }
                let mut actions = vec![ContentAction::Read];
                if request.write { actions.push(ContentAction::Write); }
                if request.share { actions.push(ContentAction::Share); }
                let permissions = Permissions::from_actions(&actions);
                let grant = ContentGrant { resource_id: request.project_id, membership_id: member.id,
                    permissions, active: true, revision: revision + 1 };
                if revision == 0 {
                    sqlx::query("INSERT INTO identity_content_grants(resource_id,membership_id,permissions,active,revision) VALUES($1,$2,$3,1,1)")
                        .bind(request.project_id.to_string()).bind(member.id.to_string())
                        .bind(permissions.bits()).execute(&mut **tx).await?;
                } else {
                    sqlx::query("UPDATE identity_content_grants SET permissions = $1,active = 1,revision = $2 WHERE resource_id = $3 AND membership_id = $4")
                        .bind(permissions.bits()).bind(grant.revision).bind(request.project_id.to_string())
                        .bind(member.id.to_string()).execute(&mut **tx).await?;
                }
                grants.push(grant);
            }
            self.audit_event(tx, Some(org), actor.identity_id, "migration_identity_mapped", member.id).await?;
            result.push(MigrationIdentityBinding { source_user_id: mapping.source_user_id.clone(),
                identity: subject, membership: member, grants });
        }
        if !mappings.is_empty() { self.bump_epoch(tx, org).await?; }
        self.actor(tx, actor).await?;
        Ok(result)
    }
}
