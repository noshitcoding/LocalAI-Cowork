use super::*;
use crate::android_devices::{device_invalid, device_writer};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction};
pub(crate) async fn private_validate(
    tx: &mut Transaction<'_, Any>,
    project: Uuid,
    user: Uuid,
    revision: i64,
    action: ContentAction,
) -> Result<()> {
    let row=sqlx::query("SELECT s.*,g.owner_id,g.permissions FROM android_private_support_sources x JOIN android_support_grants s ON s.id=x.support_id JOIN android_private_project_grants g ON g.project_id=x.project_id AND g.user_id=x.user_id WHERE x.project_id=$1 AND x.user_id=$2").bind(project.to_string()).bind(user.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    if let Some(s) = row {
        let now: i64 = match tx.backend_name() {
            "SQLite" => {
                sqlx::query_scalar("SELECT CAST((julianday('now')-2440587.5)*86400000 AS BIGINT)")
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(storage_error)?
            }
            "PostgreSQL" => sqlx::query_scalar(
                "SELECT CAST(FLOOR(EXTRACT(EPOCH FROM clock_timestamp())*1000) AS BIGINT)",
            )
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?,
            _ => return Err(Error::Forbidden),
        };
        let issuer: String = s.try_get("issuer_id").map_err(storage_error)?;
        let allowed:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_projects p JOIN identity_subjects i ON i.id=$1 WHERE p.id=$2 AND p.deleted=0 AND i.active=1 AND (p.owner_id=$1 OR EXISTS(SELECT 1 FROM android_private_project_grants g WHERE g.project_id=p.id AND g.owner_id=p.owner_id AND g.user_id=$1 AND g.active=1 AND (g.permissions&8)=8))").bind(issuer).bind(project.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
        if action != ContentAction::Read
            || allowed != 1
            || s.try_get::<i64, _>("active").map_err(storage_error)? != 1
            || s.try_get::<i64, _>("expires_at").map_err(storage_error)? <= now
            || s.try_get::<String, _>("project_id")
                .map_err(storage_error)?
                != project.to_string()
            || s.try_get::<String, _>("user_id").map_err(storage_error)? != user.to_string()
            || s.try_get::<String, _>("source_owner_id")
                .map_err(storage_error)?
                != s.try_get::<String, _>("owner_id").map_err(storage_error)?
            || s.try_get::<i64, _>("grant_revision")
                .map_err(storage_error)?
                != revision
            || s.try_get::<i64, _>("permissions").map_err(storage_error)? != 1
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}
pub(crate) async fn private_clear(
    tx: &mut Transaction<'_, Any>,
    project: Uuid,
    user: Uuid,
) -> Result<()> {
    sqlx::query("UPDATE android_support_grants SET active=0,revision=revision+1 WHERE id IN(SELECT support_id FROM android_private_support_sources WHERE project_id=$1 AND user_id=$2) AND active=1").bind(project.to_string()).bind(user.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
    sqlx::query("DELETE FROM android_private_support_sources WHERE project_id=$1 AND user_id=$2")
        .bind(project.to_string())
        .bind(user.to_string())
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    Ok(())
}
pub(crate) async fn receipt_scope(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    receipt: &Value,
) -> Result<()> {
    let scope = store
        .identity()
        .session_organization_in_transaction(tx, actor)
        .await
        .map_err(team_service::identity_error)?;
    if let Some(org) = scope
        && receipt["organization_id"].as_str() != Some(org.to_string().as_str())
    {
        return Err(Error::Forbidden);
    }
    store
        .identity()
        .require_session_action_in_transaction(tx, actor, ContentAction::Read)
        .await
        .map_err(team_service::identity_error)
}
impl Store {
    pub async fn android_support_grants(
        &self,
        actor: &Principal,
        project: Option<Uuid>,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Value> {
        if !(1..=100).contains(&limit) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let session_org = self
            .identity()
            .session_organization_in_transaction(&mut tx, actor)
            .await
            .map_err(team_service::identity_error)?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        if let Some(project) = project {
            self.android_task_project_permit(&mut tx, actor, project, ContentAction::Share)
                .await?;
        }
        let rows=sqlx::query("SELECT * FROM android_support_grants WHERE id>$1 AND (($2 IS NOT NULL AND project_id=$2) OR ($2 IS NULL AND (issuer_id=$3 OR user_id=$3))) AND ($5 IS NULL OR organization_id=$5) ORDER BY id LIMIT $4").bind(after.map(|v|v.to_string()).unwrap_or_default()).bind(project.map(|p|p.to_string())).bind(actor.identity_id().to_string()).bind(i64::from(limit)+1).bind(session_org.map(|v|v.to_string())).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let more = rows.len() > limit as usize;
        let mut items = Vec::new();
        for r in rows.iter().take(limit as usize) {
            items.push(json!({"id":r.try_get::<String,_>("id").map_err(storage_error)?,"issuer_id":r.try_get::<String,_>("issuer_id").map_err(storage_error)?,"user_id":r.try_get::<String,_>("user_id").map_err(storage_error)?,"project_id":r.try_get::<String,_>("project_id").map_err(storage_error)?,"organization_id":r.try_get::<Option<String>,_>("organization_id").map_err(storage_error)?,"revision":r.try_get::<i64,_>("revision").map_err(storage_error)?,"grant_revision":r.try_get::<i64,_>("grant_revision").map_err(storage_error)?,"active":r.try_get::<i64,_>("active").map_err(storage_error)?==1,"expires_at":r.try_get::<i64,_>("expires_at").map_err(storage_error)?,"reason":r.try_get::<String,_>("reason").map_err(storage_error)?}));
        }
        let next = if more {
            items.last().map(|v| v["id"].clone())
        } else {
            None
        };
        if let Some(project) = project {
            self.android_task_project_permit(&mut tx, actor, project, ContentAction::Share)
                .await?;
        }
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"items":items,"next_cursor":next}))
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_support_create: Keep requesting actor, recipient/project, expected revision, expiry and support reason explicit"
    )]
    pub async fn android_support_create(
        &self,
        actor: &Principal,
        project: Uuid,
        user: Uuid,
        expected: i64,
        expires: i64,
        reason: &str,
        request: Uuid,
    ) -> Result<Value> {
        if request.is_nil() {
            return Err(device_invalid());
        }
        let sha = canonical_hash(
            &json!({"project_id":project,"user_id":user,"expected_revision":expected,"expires_at":expires,"reason":reason,"request_id":request}),
        );
        let key = format!("support-create:{request}");
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let session_org = self
            .identity()
            .session_organization_in_transaction(&mut tx, actor)
            .await
            .map_err(team_service::identity_error)?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        if let Some(v) = crate::android_administration_control::original(
            &mut tx,
            actor.identity_id(),
            &key,
            "support_create",
            &sha,
        )
        .await?
        {
            receipt_scope(self, &mut tx, actor, &v).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let id = Uuid::new_v4();
        let org: Option<String> =
            sqlx::query_scalar("SELECT organization_id FROM tenant_projects WHERE id=$1")
                .bind(project.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        if session_org.is_some() && org.as_deref() != session_org.map(|v| v.to_string()).as_deref()
        {
            return Err(Error::Forbidden);
        }
        let receipt_org = org.clone();
        let granted = if let Some(org) = org {
            let org = Uuid::parse_str(&org).map_err(storage_error)?;
            self.identity()
                .android_create_tenant_support_in_transaction(
                    &mut tx, actor, org, project, user, expected, expires, reason, id,
                )
                .await
                .map_err(team_service::identity_error)?
                .1
        } else {
            crate::android_private_sharing::authorize(
                self,
                &mut tx,
                actor,
                project,
                ContentAction::Share,
            )
            .await?;
            let owner = crate::android_private_sharing::source_owner(&mut tx, project).await?;
            if user == owner
                || expected < 0
                || expires <= now
                || expires > now + 7 * 24 * 60 * 60 * 1000
                || reason.trim().is_empty()
                || reason.len() > 2048
            {
                return Err(device_invalid());
            }
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM identity_subjects WHERE id=$1 AND active=1",
            )
            .bind(user.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
            if exists != 1 {
                return Err(Error::NotFound);
            }
            let old=sqlx::query("SELECT revision,active FROM android_private_project_grants WHERE project_id=$1 AND user_id=$2").bind(project.to_string()).bind(user.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
            let revision = old
                .as_ref()
                .map(|r| r.try_get::<i64, _>("revision").map_err(storage_error))
                .transpose()?
                .unwrap_or(0);
            let managed:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_support_sources WHERE project_id=$1 AND user_id=$2").bind(project.to_string()).bind(user.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if revision != expected
                || old
                    .as_ref()
                    .is_some_and(|r| r.try_get::<i64, _>("active").ok() == Some(1))
                    && managed == 0
            {
                return Err(Error::Conflict);
            }
            private_clear(&mut tx, project, user).await?;
            let next = revision.checked_add(1).ok_or(Error::Conflict)?;
            self.identity()
                .android_record_private_sharing_in_transaction(&mut tx, actor, project, user)
                .await
                .map_err(team_service::identity_error)?;
            sqlx::query("INSERT INTO android_private_project_grants(project_id,owner_id,user_id,permissions,revision,active,updated_at) VALUES($1,$2,$3,1,$4,1,$5) ON CONFLICT(project_id,user_id) DO UPDATE SET permissions=1,revision=$4,active=1,updated_at=$5 WHERE android_private_project_grants.revision=$6 AND android_private_project_grants.owner_id=$2").bind(project.to_string()).bind(owner.to_string()).bind(user.to_string()).bind(next).bind(now).bind(revision).execute(&mut *tx).await.map_err(storage_error)?;
            sqlx::query("INSERT INTO android_support_grants(id,issuer_id,user_id,project_id,source_owner_id,grant_revision,expires_at,active,revision,reason,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,1,1,$8,$9)").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(user.to_string()).bind(project.to_string()).bind(owner.to_string()).bind(next).bind(expires).bind(reason).bind(now).execute(&mut *tx).await.map_err(storage_error)?;
            sqlx::query("INSERT INTO android_private_support_sources(project_id,user_id,support_id) VALUES($1,$2,$3)").bind(project.to_string()).bind(user.to_string()).bind(id.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
            next
        };
        let receipt = json!({"schema_version":2,"id":id,"organization_id":receipt_org,"project_id":project,"user_id":user,"issuer_id":actor.identity_id(),"revision":1,"grant_revision":granted,"expires_at":expires,"permissions":["read"],"request_id":request,"request_sha256":sha});
        crate::android_administration_control::remember(
            &mut tx,
            actor.identity_id(),
            &key,
            "support_create",
            &sha,
            &receipt,
            now,
        )
        .await?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Share)
            .await
            .map_err(team_service::identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn android_support_revoke(
        &self,
        actor: &Principal,
        id: Uuid,
        expected: i64,
        request: Uuid,
    ) -> Result<Value> {
        if request.is_nil() || expected <= 0 {
            return Err(device_invalid());
        }
        let sha =
            canonical_hash(&json!({"id":id,"expected_revision":expected,"request_id":request}));
        let key = format!("support-revoke:{request}");
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let session_org = self
            .identity()
            .session_organization_in_transaction(&mut tx, actor)
            .await
            .map_err(team_service::identity_error)?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        if let Some(v) = crate::android_administration_control::original(
            &mut tx,
            actor.identity_id(),
            &key,
            "support_revoke",
            &sha,
        )
        .await?
        {
            receipt_scope(self, &mut tx, actor, &v).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let r = sqlx::query("SELECT * FROM android_support_grants WHERE id=$1")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        if session_org.is_some()
            && r.try_get::<Option<String>, _>("organization_id")
                .map_err(storage_error)?
                .as_deref()
                != session_org.map(|v| v.to_string()).as_deref()
        {
            return Err(Error::Forbidden);
        }
        if r.try_get::<Option<String>, _>("organization_id")
            .map_err(storage_error)?
            .is_some()
        {
            self.identity()
                .android_revoke_tenant_support_in_transaction(&mut tx, actor, id, expected)
                .await
                .map_err(team_service::identity_error)?;
        } else {
            let project = Uuid::parse_str(
                &r.try_get::<String, _>("project_id")
                    .map_err(storage_error)?,
            )
            .map_err(storage_error)?;
            let user = Uuid::parse_str(&r.try_get::<String, _>("user_id").map_err(storage_error)?)
                .map_err(storage_error)?;
            crate::android_private_sharing::authorize(
                self,
                &mut tx,
                actor,
                project,
                ContentAction::Share,
            )
            .await?;
            if r.try_get::<i64, _>("revision").map_err(storage_error)? != expected {
                return Err(Error::Conflict);
            }
            let original:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_support_sources WHERE project_id=$1 AND user_id=$2 AND support_id=$3").bind(project.to_string()).bind(user.to_string()).bind(id.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if original == 1 {
                sqlx::query("UPDATE android_private_project_grants SET active=0,revision=revision+1 WHERE project_id=$1 AND user_id=$2 AND revision=$3 AND permissions=1").bind(project.to_string()).bind(user.to_string()).bind(r.try_get::<i64,_>("grant_revision").map_err(storage_error)?).execute(&mut *tx).await.map_err(storage_error)?;
                private_clear(&mut tx, project, user).await?;
            } else {
                sqlx::query("UPDATE android_support_grants SET active=0,revision=revision+1 WHERE id=$1 AND revision=$2").bind(id.to_string()).bind(expected).execute(&mut *tx).await.map_err(storage_error)?;
            }
            self.identity()
                .android_record_private_sharing_in_transaction(&mut tx, actor, project, user)
                .await
                .map_err(team_service::identity_error)?;
        }
        let receipt = json!({"id":id,"organization_id":r.try_get::<Option<String>,_>("organization_id").map_err(storage_error)?,"project_id":r.try_get::<String,_>("project_id").map_err(storage_error)?,"revision":expected+1,"active":false,"request_id":request,"request_sha256":sha});
        crate::android_administration_control::remember(
            &mut tx,
            actor.identity_id(),
            &key,
            "support_revoke",
            &sha,
            &receipt,
            now,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
