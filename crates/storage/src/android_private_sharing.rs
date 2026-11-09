//! Private content uses its real source owner; executing users retain their own secrets and run journals.
use super::*;
use crate::android_devices::{device_invalid, device_writer};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction};
use team_service::identity_error;

fn bit(action: ContentAction) -> i64 {
    match action {
        ContentAction::Read => 1,
        ContentAction::Write => 2,
        ContentAction::Execute => 4,
        ContentAction::Share => 8,
    }
}
fn roles(bits: i64) -> Vec<&'static str> {
    [(1, "read"), (2, "write"), (4, "execute"), (8, "share")]
        .into_iter()
        .filter(|(b, _)| bits & b == *b)
        .map(|(_, s)| s)
        .collect()
}
pub(crate) async fn source_owner(tx: &mut Transaction<'_, Any>, project: Uuid) -> Result<Uuid> {
    let owner: String=sqlx::query_scalar("SELECT p.owner_id FROM android_personal_projects p JOIN identity_subjects i ON i.id=p.owner_id WHERE p.id=$1 AND p.deleted=0 AND i.active=1").bind(project.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    Uuid::parse_str(&owner).map_err(storage_error)
}
/// Returns an original grant stamp only for a non-owner. Owner paths preserve absent fields.
pub(crate) async fn stamp(
    tx: &mut Transaction<'_, Any>,
    actor: Uuid,
    project: Uuid,
    action: ContentAction,
) -> Result<Option<Value>> {
    let owner = source_owner(tx, project).await?;
    if owner == actor {
        return Ok(None);
    }
    let row=sqlx::query("SELECT permissions,revision,owner_id FROM android_private_project_grants WHERE project_id=$1 AND user_id=$2 AND active=1").bind(project.to_string()).bind(actor.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let permissions: i64 = row.try_get("permissions").map_err(storage_error)?;
    if row
        .try_get::<String, _>("owner_id")
        .map_err(storage_error)?
        != owner.to_string()
        || permissions & bit(action) != bit(action)
    {
        return Err(Error::Forbidden);
    }
    crate::android_support_grants::private_validate(
        tx,
        project,
        actor,
        row.try_get::<i64, _>("revision").map_err(storage_error)?,
        action,
    )
    .await?;
    Ok(Some(
        json!({"format":1,"project_id":project,"source_owner_id":owner,"actor_id":actor,"grant_revision":row.try_get::<i64,_>("revision").map_err(storage_error)?,"permissions":permissions}),
    ))
}
/// Exact original authority, never a current grant substituted for a missing old stamp.
pub(crate) async fn validate_original(
    tx: &mut Transaction<'_, Any>,
    actor: Uuid,
    project: Uuid,
    input: &Value,
    action: ContentAction,
) -> Result<()> {
    let actual = stamp(tx, actor, project, action).await?;
    let original = input.get("private_project_access").filter(|v| !v.is_null());
    if actual.as_ref() != original {
        return Err(Error::Forbidden);
    }
    Ok(())
}
pub(crate) async fn authorize(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    project: Uuid,
    action: ContentAction,
) -> Result<Value> {
    store
        .identity()
        .require_global_session_in_transaction(tx, actor)
        .await
        .map_err(identity_error)?;
    store
        .identity()
        .require_session_action_in_transaction(tx, actor, action)
        .await
        .map_err(identity_error)?;
    stamp(tx, actor.identity_id(), project, action).await?;
    let text: String = sqlx::query_scalar(
        "SELECT payload FROM android_personal_projects WHERE id=$1 AND deleted=0",
    )
    .bind(project.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    let value: Value = parse(&text)?;
    if value["owner_user_id"].as_str()
        != Some(source_owner(tx, project).await?.to_string().as_str())
        || value["id"] != json!(project)
    {
        return Err(Error::Forbidden);
    }
    store
        .identity()
        .require_session_action_in_transaction(tx, actor, action)
        .await
        .map_err(identity_error)?;
    Ok(value)
}
impl Store {
    pub async fn android_private_project_exists(&self, project: Uuid) -> Result<bool> {
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_projects WHERE id=$1")
                .bind(project.to_string())
                .fetch_one(&self.pool)
                .await
                .map_err(storage_error)?;
        Ok(n == 1)
    }
    pub async fn android_private_project_access_stamp(
        &self,
        actor: &Principal,
        project: Uuid,
    ) -> Result<Option<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        authorize(self, &mut tx, actor, project, ContentAction::Execute).await?;
        let result = stamp(
            &mut tx,
            actor.identity_id(),
            project,
            ContentAction::Execute,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_private_project_members(
        &self,
        actor: &Principal,
        project: Uuid,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Value> {
        if !(1..=100).contains(&limit) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        authorize(self, &mut tx, actor, project, ContentAction::Share).await?;
        let owner = source_owner(&mut tx, project).await?;
        let rows=sqlx::query("SELECT g.user_id,g.permissions,g.revision,g.active,i.display_name,i.active AS identity_active FROM android_private_project_grants g JOIN identity_subjects i ON i.id=g.user_id WHERE g.project_id=$1 AND g.owner_id=$2 AND g.user_id>$3 ORDER BY g.user_id LIMIT $4").bind(project.to_string()).bind(owner.to_string()).bind(after.map(|v|v.to_string()).unwrap_or_default()).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let more = rows.len() > limit as usize;
        let mut items = Vec::new();
        let mut last = None;
        for row in rows.into_iter().take(limit as usize) {
            let user =
                Uuid::parse_str(&row.try_get::<String, _>("user_id").map_err(storage_error)?)
                    .map_err(storage_error)?;
            let permissions: i64 = row.try_get("permissions").map_err(storage_error)?;
            let mut active = row.try_get::<i64, _>("active").map_err(storage_error)? == 1
                && row
                    .try_get::<i64, _>("identity_active")
                    .map_err(storage_error)?
                    == 1;
            if active {
                match crate::android_support_grants::private_validate(
                    &mut tx,
                    project,
                    user,
                    row.try_get("revision").map_err(storage_error)?,
                    ContentAction::Read,
                )
                .await
                {
                    Ok(()) => {}
                    Err(Error::Forbidden) => active = false,
                    Err(e) => return Err(e),
                }
            }
            last = Some(user);
            items.push(json!({"project_id":project,"source_owner_id":owner,"user_id":user,"display_name":row.try_get::<String,_>("display_name").map_err(storage_error)?,"role":match permissions{1=>"viewer",5=>"runner",15=>"editor",_=>return Err(Error::Forbidden)},"permissions":roles(permissions),"revision":row.try_get::<i64,_>("revision").map_err(storage_error)?,"active":active}));
        }
        authorize(self, &mut tx, actor, project, ContentAction::Share).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"schema_version":2,"items":items,"next_cursor":if more{last}else{None}}))
    }
    pub async fn android_private_sharing_original(
        &self,
        actor: &Principal,
        project: Uuid,
        key: &str,
        sha: &str,
    ) -> Result<Value> {
        if key.is_empty() || key.len() > 200 || sha.len() != 64 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        authorize(self, &mut tx, actor, project, ContentAction::Read).await?;
        let r=sqlx::query("SELECT project_id,request_sha256,receipt FROM android_private_sharing_operations WHERE actor_id=$1 AND operation_key=$2").bind(actor.identity_id().to_string()).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        if r.try_get::<String, _>("project_id")
            .map_err(storage_error)?
            != project.to_string()
            || r.try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != sha
        {
            return Err(Error::Conflict);
        }
        let receipt: Value = parse(&r.try_get::<String, _>("receipt").map_err(storage_error)?)?;
        if receipt["request_sha256"] != sha {
            return Err(Error::Forbidden);
        }
        authorize(self, &mut tx, actor, project, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn android_private_project_member_write(
        &self,
        actor: &Principal,
        project: Uuid,
        user: Uuid,
        role: Option<&str>,
        expected: Option<i64>,
        key: Option<&str>,
    ) -> Result<Value> {
        let bits = match role {
            None => 1,
            Some("viewer") => 1,
            Some("runner") => 5,
            Some("editor") => 15,
            _ => return Err(device_invalid()),
        };
        if key.is_some_and(|k| k.is_empty() || k.len() > 200 || k.chars().any(char::is_control))
            || expected.is_some_and(|v| v < 0)
        {
            return Err(device_invalid());
        }
        let digest = canonical_hash(
            &json!({"domain":"private-project-sharing/v1","project":project,"user":user,"role":role,"expected_revision":expected}),
        );
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        authorize(self, &mut tx, actor, project, ContentAction::Read).await?;
        if let Some(key) = key
            && let Some(row)=sqlx::query("SELECT project_id,request_sha256,receipt FROM android_private_sharing_operations WHERE actor_id=$1 AND operation_key=$2").bind(actor.identity_id().to_string()).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)?{if row.try_get::<String,_>("project_id").map_err(storage_error)?!=project.to_string()||row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=digest{return Err(Error::Conflict)}let receipt=parse(&row.try_get::<String,_>("receipt").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(receipt)}
        authorize(self, &mut tx, actor, project, ContentAction::Share).await?;
        let owner = source_owner(&mut tx, project).await?;
        if user == owner {
            return Err(Error::Forbidden);
        }
        let active: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM identity_subjects WHERE id=$1 AND active=1")
                .bind(user.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if active != 1 {
            return Err(Error::NotFound);
        }
        let old=sqlx::query("SELECT revision,owner_id,permissions,active FROM android_private_project_grants WHERE project_id=$1 AND user_id=$2").bind(project.to_string()).bind(user.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let previous = old
            .as_ref()
            .map(|r| r.try_get::<i64, _>("revision").map_err(storage_error))
            .transpose()?
            .unwrap_or(0);
        if expected.is_some_and(|v| v != previous)
            || old.as_ref().is_some_and(|r| {
                r.try_get::<String, _>("owner_id").ok().as_deref()
                    != Some(owner.to_string().as_str())
            })
        {
            return Err(Error::Conflict);
        }
        if old.is_none() && role.is_none() {
            return Err(Error::NotFound);
        }
        let managed:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_support_sources WHERE project_id=$1 AND user_id=$2").bind(project.to_string()).bind(user.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let changed = managed > 0
            || old.as_ref().is_none_or(|r| {
                r.try_get::<i64, _>("permissions").ok() != Some(bits)
                    || r.try_get::<i64, _>("active").ok() != Some(i64::from(role.is_some()))
            });
        let revision = if changed {
            previous.checked_add(1).ok_or(Error::Conflict)?
        } else {
            previous
        };
        // Authority is evaluated after all preceding SQL waits; the writer lock serializes grant mutation.
        authorize(self, &mut tx, actor, project, ContentAction::Share).await?;
        if changed {
            self.identity()
                .android_record_private_sharing_in_transaction(&mut tx, actor, project, user)
                .await
                .map_err(identity_error)?;
        }
        crate::android_support_grants::private_clear(&mut tx, project, user).await?;
        let n=sqlx::query("INSERT INTO android_private_project_grants(project_id,owner_id,user_id,permissions,revision,active,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(project_id,user_id) DO UPDATE SET permissions=excluded.permissions,revision=excluded.revision,active=excluded.active,updated_at=excluded.updated_at WHERE android_private_project_grants.revision=$8 AND android_private_project_grants.owner_id=$2").bind(project.to_string()).bind(owner.to_string()).bind(user.to_string()).bind(bits).bind(revision).bind(i64::from(role.is_some())).bind(current).bind(previous).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if n != 1 {
            return Err(Error::Conflict);
        }
        let receipt = json!({"schema_version":2,"project_id":project,"source_owner_id":owner,"user_id":user,"permissions":roles(bits),"revision":revision,"active":role.is_some(),"request_sha256":digest});
        if let Some(key) = key {
            sqlx::query("INSERT INTO android_private_sharing_operations(actor_id,operation_key,project_id,request_sha256,receipt,created_at) VALUES($1,$2,$3,$4,$5,$6)").bind(actor.identity_id().to_string()).bind(key).bind(project.to_string()).bind(digest).bind(json_string(&receipt)?).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        }
        self.identity()
            .require_global_session_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
