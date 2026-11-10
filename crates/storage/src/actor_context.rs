//! Identity-owned content shares the authenticated immutable metadata journal.
use super::*;
use crate::android_devices::{device_invalid, device_writer};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction, any::AnyRow};
async fn permit(
    s: &Store,
    tx: &mut Transaction<'_, Any>,
    a: &Principal,
    project: Option<Uuid>,
    action: ContentAction,
) -> Result<()> {
    s.identity()
        .require_global_session_in_transaction(tx, a)
        .await
        .map_err(team_service::identity_error)?;
    s.identity()
        .require_session_action_in_transaction(tx, a, action)
        .await
        .map_err(team_service::identity_error)?;
    if let Some(p) = project {
        s.android_personal_permit(tx, a, p, action).await?;
    }
    Ok(())
}
fn kind(k: &str) -> Result<&str> {
    match k {
        "skills" => Ok("skill"),
        "memories" => Ok("memory"),
        _ => Err(device_invalid()),
    }
}
fn record(r: &AnyRow) -> Result<ActorContextRecord> {
    let raw: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    Ok(ActorContextRecord {
        id: crate::android_personal::wire_uuid(&raw, "entity_id")?,
        revision: raw["revision"].as_i64().ok_or(Error::Forbidden)?,
        etag: raw["etag"].as_str().ok_or(Error::Forbidden)?.into(),
        deleted: raw["tombstone"] == true,
        project_id: r
            .try_get::<Option<String>, _>("project_id")
            .map_err(storage_error)?
            .map(|id| Uuid::parse_str(&id).map_err(storage_error))
            .transpose()?,
        value: raw["payload"].clone(),
    })
}
fn text(v: &str, max: usize, required: bool) -> Result<()> {
    if v.len() > max || v.contains('\0') || (required && v.trim().is_empty()) {
        return Err(device_invalid());
    }
    Ok(())
}
impl Store {
    pub async fn actor_context(
        &self,
        a: &Principal,
        k: &str,
        project: Option<Uuid>,
    ) -> Result<Vec<ActorContextRecord>> {
        let k = kind(k)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, a, project, ContentAction::Read).await?;
        let rows=sqlx::query("SELECT payload,project_id FROM android_wire_sync_current WHERE owner_id=$1 AND entity_type=$2 AND team_id IS NULL AND tombstone=0 AND (project_id IS NULL OR project_id=$3) ORDER BY entity_id LIMIT 1001").bind(a.identity_id().to_string()).bind(k).bind(project.map(|p|p.to_string())).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 1000 {
            return Err(Error::BudgetExceeded);
        }
        let values = rows.iter().map(record).collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn actor_context_mutate(
        &self,
        a: &Principal,
        k: &str,
        id: Uuid,
        raw: &Value,
        delete: bool,
    ) -> Result<ActorContextRecord> {
        let k = kind(k)?;
        let (request, expected, mut value) = if delete {
            let q: DeleteActorContext =
                serde_json::from_value(raw.clone()).map_err(|_| device_invalid())?;
            (q.request_id, q.expected_revision, Value::Null)
        } else if k == "skill" {
            let q: WriteActorSkill =
                serde_json::from_value(raw.clone()).map_err(|_| device_invalid())?;
            text(&q.value.name, 200, true)?;
            text(&q.value.description, 16000, false)?;
            text(&q.value.prompt_template, 64000, true)?;
            if let Some(trigger) = &q.value.trigger_pattern {
                text(trigger, 1000, true)?;
            }
            (
                q.request_id,
                q.expected_revision,
                serde_json::to_value(q.value).map_err(storage_error)?,
            )
        } else {
            let q: WriteActorMemory =
                serde_json::from_value(raw.clone()).map_err(|_| device_invalid())?;
            text(&q.value.key, 200, true)?;
            text(&q.value.content, 64000, true)?;
            text(&q.value.category, 80, true)?;
            if q.value
                .confidence
                .is_some_and(|n| !n.is_finite() || !(0.0..=1.0).contains(&n))
            {
                return Err(device_invalid());
            }
            (
                q.request_id,
                q.expected_revision,
                serde_json::to_value(q.value).map_err(storage_error)?,
            )
        };
        if request.is_nil() || id.is_nil() || expected < 0 {
            return Err(device_invalid());
        }
        let request_sha = canonical_hash(&("actor-context/1", k, id, raw, delete));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let stamp = device_writer(&mut tx).await?;
        permit(self, &mut tx, a, None, ContentAction::Write).await?;
        if let Some((sha,receipt))=sqlx::query_as::<_,(String,String)>("SELECT request_sha256,receipt FROM android_wire_sync_receipts WHERE owner_id=$1 AND operation_id=$2").bind(a.identity_id().to_string()).bind(request.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?{
            if sha!=request_sha{return Err(Error::Conflict);}let result:ActorContextRecord=parse(&receipt)?;permit(self,&mut tx,a,result.project_id,ContentAction::Write).await?;tx.commit().await.map_err(storage_error)?;return Ok(result);
        }
        let old=sqlx::query("SELECT payload,project_id,revision FROM android_wire_sync_current WHERE owner_id=$1 AND entity_type=$2 AND entity_id=$3 AND team_id IS NULL").bind(a.identity_id().to_string()).bind(k).bind(id.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let old = old.as_ref().map(record).transpose()?;
        if old.is_none() {
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_wire_sync_current WHERE owner_id=$1 AND entity_type=$2 AND team_id IS NULL AND tombstone=0").bind(a.identity_id().to_string()).bind(k).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if count >= 1000 {
                return Err(Error::BudgetExceeded);
            }
        }
        if old.as_ref().map_or(0, |r| r.revision) != expected
            || delete && old.as_ref().is_none_or(|v| v.deleted)
        {
            return Err(Error::Conflict);
        }
        let project = if delete {
            old.as_ref().and_then(|v| v.project_id)
        } else {
            value["project_id"]
                .as_str()
                .map(|s| Uuid::parse_str(s).map_err(|_| device_invalid()))
                .transpose()?
        };
        if old.as_ref().is_some_and(|old| old.project_id != project) {
            return Err(Error::Validation(
                "Context scope changes require an explicit new copy".into(),
            ));
        }
        permit(self, &mut tx, a, project, ContentAction::Write).await?;
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM android_wire_sync_receipts WHERE owner_id=$1")
                .bind(a.identity_id().to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if count >= 100000 {
            return Err(Error::BudgetExceeded);
        }
        if !delete && k == "memory" {
            value["scope"] = json!(if project.is_some() { "project" } else { "user" });
            value["scope_ref"] = json!(project);
        }
        if !delete && k == "skill" && value["auto_learn"] == true {
            // Legacy metadata may already contain an inert auto_learn flag.
            // Only a new explicit skill save opts into this implemented path.
            value["auto_learn_policy"] = json!("reviewed-outcomes/1");
        }
        let revision = expected.checked_add(1).ok_or(Error::BudgetExceeded)?;
        let entity = crate::android_wire_sync::record(
            &mut tx,
            a.identity_id(),
            k,
            id,
            None,
            project,
            revision,
            &value,
            delete,
            stamp,
        )
        .await?;
        let result = ActorContextRecord {
            id,
            revision,
            etag: entity["etag"].as_str().ok_or(Error::Forbidden)?.into(),
            deleted: delete,
            project_id: project,
            value: if delete { Value::Null } else { value },
        };
        sqlx::query("INSERT INTO android_wire_sync_receipts(owner_id,operation_id,request_sha256,receipt) VALUES($1,$2,$3,$4)").bind(a.identity_id().to_string()).bind(request.to_string()).bind(request_sha).bind(json_string(&result)?).execute(&mut *tx).await.map_err(storage_error)?;
        permit(self, &mut tx, a, project, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
