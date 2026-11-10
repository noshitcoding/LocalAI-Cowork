//! Product project locators over current Identity authority; never NativeOwner.
use super::*;
use crate::team_service::identity_error;
use cowork_identity::Principal;
async fn organization(store: &Store, project: Uuid) -> Result<Uuid> {
    let id: Option<String> =
        sqlx::query_scalar("SELECT organization_id FROM tenant_projects WHERE id=$1")
            .bind(project.to_string())
            .fetch_optional(&store.pool)
            .await
            .map_err(storage_error)?;
    id.and_then(|s| Uuid::parse_str(&s).ok())
        .ok_or(Error::NotFound)
}
impl Store {
    pub async fn android_project_member_page(
        &self,
        actor: &Principal,
        project: Uuid,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<AndroidProjectMemberPage> {
        if self.android_private_project_exists(project).await? {
            return Err(Error::Validation(
                "Use the private-member page for personal projects".into(),
            ));
        }
        let org = organization(self, project).await?;
        self.identity()
            .android_project_members(actor, org, project, after, limit)
            .await
            .map_err(identity_error)
    }
    pub async fn android_project_member_write(
        &self,
        actor: &Principal,
        project: Uuid,
        user: Uuid,
        role: Option<&str>,
        expected: Option<i64>,
        key: Option<&str>,
    ) -> Result<Value> {
        if self.android_private_project_exists(project).await? {
            return self
                .android_private_project_member_write(actor, project, user, role, expected, key)
                .await;
        }
        let org = organization(self, project).await?;
        let grant = self
            .identity()
            .android_set_project_member(actor, org, project, user, role, expected, key)
            .await
            .map_err(identity_error)?;
        Ok(
            json!({"schema_version":2,"project_id":project,"user_id":user,"membership_id":grant.membership_id,"permissions":grant.permissions,"revision":grant.revision,"active":grant.active}),
        )
    }
    pub async fn android_content_access(
        &self,
        actor: &Principal,
        project: Option<Uuid>,
    ) -> Result<AndroidContentAccess> {
        if let Some(project) = project {
            if self.android_private_project_exists(project).await? {
                let mut tx = self.pool.begin().await.map_err(storage_error)?;
                crate::android_private_sharing::authorize(
                    self,
                    &mut tx,
                    actor,
                    project,
                    cowork_identity::ContentAction::Read,
                )
                .await?;
                let stamp = crate::android_private_sharing::stamp(
                    &mut tx,
                    actor.identity_id(),
                    project,
                    cowork_identity::ContentAction::Read,
                )
                .await?;
                let bits = stamp
                    .as_ref()
                    .and_then(|v| v["permissions"].as_i64())
                    .unwrap_or(15);
                let permissions = [(1, "read"), (2, "write"), (4, "execute"), (8, "share")]
                    .into_iter()
                    .filter(|(bit, _)| bits & bit == *bit)
                    .map(|(_, v)| v.to_owned())
                    .collect();
                let result = AndroidContentAccess {
                    schema_version: 2,
                    actor_user_id: actor.identity_id(),
                    organisation_id: None,
                    project_id: Some(project),
                    managed: false,
                    instance_viewer: bits == 1,
                    local_membership_allowed: true,
                    automatic_content_access: false,
                    support_access: sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM android_private_support_sources WHERE project_id=$1 AND user_id=$2").bind(project.to_string()).bind(actor.identity_id().to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?==1,
                    permissions,
                };
                tx.commit().await.map_err(storage_error)?;
                return Ok(result);
            }
        }
        let target = match project {
            Some(project) => Some((organization(self, project).await?, project)),
            None => None,
        };
        self.identity()
            .android_content_access(actor, target)
            .await
            .map_err(identity_error)
    }
}
