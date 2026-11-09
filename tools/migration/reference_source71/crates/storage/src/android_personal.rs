//! V1 wire content owned by the actual identity. Federated team sessions cannot cross into this scope.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction};
use team_service::identity_error;

pub(crate) fn wire_uuid(v: &Value, key: &str) -> Result<Uuid> {
    v[key]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(device_invalid)
}
fn text(v: &Value, field: &str, max: usize, empty: bool) -> Result<String> {
    let s = v[field].as_str().ok_or_else(device_invalid)?;
    if s.len() > max || (!empty && s.trim().is_empty()) || s.chars().any(|c| c == '\0') {
        return Err(device_invalid());
    }
    Ok(s.to_owned())
}
fn revision(v: &Value, field: &str) -> Result<i64> {
    v[field]
        .as_i64()
        .filter(|v| *v > 0)
        .ok_or_else(device_invalid)
}
fn bump(v: &mut Value, revision: i64, current: i64) -> Result<()> {
    v["revision"] = json!(revision);
    v["etag"] = json!(format!("{}:{revision}", wire_uuid(v, "id")?));
    v["updated_at"] = json!(device_iso(current)?);
    Ok(())
}
impl Store {
    pub(crate) async fn android_personal_permit(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        id: Uuid,
        action: ContentAction,
    ) -> Result<Value> {
        self.identity()
            .require_global_session_in_transaction(tx, actor)
            .await
            .map_err(identity_error)?;
        self.identity()
            .require_session_action_in_transaction(tx, actor, action)
            .await
            .map_err(identity_error)?;
        let row = sqlx::query(
            "SELECT owner_id,payload,deleted FROM android_personal_projects WHERE id=$1",
        )
        .bind(id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        if row
            .try_get::<String, _>("owner_id")
            .map_err(storage_error)?
            != actor.identity_id().to_string()
        {
            return Err(Error::Forbidden);
        }
        if row.try_get::<i64, _>("deleted").map_err(storage_error)? != 0 {
            return Err(Error::NotFound);
        }
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)
    }
    pub async fn android_personal_project(
        &self,
        actor: &Principal,
        id: Uuid,
    ) -> Result<Option<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_projects WHERE id=$1")
                .bind(id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if exists == 0 {
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        }
        let v = self
            .android_personal_permit(&mut tx, actor, id, ContentAction::Read)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(v))
    }
    pub async fn android_personal_projects(&self, actor: &Principal) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        if self
            .identity()
            .session_organization_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?
            .is_some()
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(Vec::new());
        }
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(identity_error)?;
        let rows=sqlx::query("SELECT payload FROM android_personal_projects WHERE owner_id=$1 AND deleted=0 ORDER BY id LIMIT 2000").bind(actor.identity_id().to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let values = rows
            .iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<Value>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_personal_create_project(
        &self,
        actor: &Principal,
        input: Value,
    ) -> Result<Value> {
        if input["privacy"] != "private_local" || !input["team_id"].is_null() {
            return Err(device_invalid());
        }
        let name = text(&input, "name", 200, false)?;
        let description = text(&input, "description", 64 * 1024, true)?;
        let policy = input["policy"].clone();
        if json_string(&policy)?.len() > 64 * 1024 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .require_global_session_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Write)
            .await
            .map_err(identity_error)?;
        let current = device_writer(&mut tx).await?;
        let id = Uuid::new_v4();
        let v = json!({"schema_version":2,"id":id,"revision":1,"etag":format!("{id}:1"),"owner_user_id":actor.identity_id(),"team_id":null,"privacy":"private_local","name":name,"description":description,"preferred_executor_target":input["preferred_executor_target"],"current_version_id":null,"policy":policy,"created_at":device_iso(current)?,"updated_at":device_iso(current)?,"deleted_at":null});
        sqlx::query("INSERT INTO android_personal_projects(id,owner_id,revision,payload,deleted) VALUES($1,$2,1,$3,0)").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(json_string(&v)?).execute(&mut *tx).await.map_err(storage_error)?;
        crate::android_wire_sync::record(
            &mut tx,
            actor.identity_id(),
            "project",
            id,
            None,
            Some(id),
            1,
            &v,
            false,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_personal_update_project(
        &self,
        actor: &Principal,
        id: Uuid,
        input: Value,
        delete: bool,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let mut v = self
            .android_personal_permit(&mut tx, actor, id, ContentAction::Write)
            .await?;
        let old = revision(&input, "expected_revision")?;
        if revision(&v, "revision")? != old {
            return Err(Error::Conflict);
        }
        if delete {
            crate::android_wire_materialization::delete_project_children(
                &mut tx,
                actor.identity_id(),
                id,
                current,
            )
            .await?;
            v["deleted_at"] = json!(device_iso(current)?);
        } else {
            v["name"] = json!(text(&input, "name", 200, false)?);
            v["description"] = json!(text(&input, "description", 64 * 1024, true)?);
            v["preferred_executor_target"] = input["preferred_executor_target"].clone();
            if json_string(&input["policy"])?.len() > 64 * 1024 {
                return Err(device_invalid());
            }
            v["policy"] = input["policy"].clone();
        }
        bump(&mut v, old + 1, current)?;
        let n=sqlx::query("UPDATE android_personal_projects SET revision=$1,payload=$2,deleted=$3 WHERE id=$4 AND revision=$5").bind(old+1).bind(json_string(&v)?).bind(i64::from(delete)).bind(id.to_string()).bind(old).execute(&mut *tx).await.map_err(storage_error)?;
        if n.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        crate::android_wire_sync::record(
            &mut tx,
            actor.identity_id(),
            "project",
            id,
            None,
            Some(id),
            old + 1,
            &v,
            delete,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_personal_threads(
        &self,
        actor: &Principal,
        project: Uuid,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        let rows=sqlx::query("SELECT payload FROM android_personal_threads WHERE project_id=$1 AND owner_id=$2 AND deleted=0 ORDER BY id LIMIT 2000").bind(project.to_string()).bind(actor.identity_id().to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let values = rows
            .iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<Value>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_personal_thread(
        &self,
        actor: &Principal,
        id: Uuid,
    ) -> Result<Option<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let row = sqlx::query(
            "SELECT project_id,owner_id,payload,deleted FROM android_personal_threads WHERE id=$1",
        )
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
        let Some(row) = row else {
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        };
        if row
            .try_get::<String, _>("owner_id")
            .map_err(storage_error)?
            != actor.identity_id().to_string()
            || row.try_get::<i64, _>("deleted").map_err(storage_error)? != 0
        {
            return Err(Error::NotFound);
        }
        let project = Uuid::parse_str(
            &row.try_get::<String, _>("project_id")
                .map_err(storage_error)?,
        )
        .map_err(storage_error)?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        let value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(value))
    }
    pub async fn android_personal_create_thread(
        &self,
        actor: &Principal,
        input: Value,
        key: Option<String>,
    ) -> Result<Value> {
        let project = wire_uuid(&input, "project_id")?;
        let title = text(&input, "title", 200, false)?;
        if !input["forked_from_thread_id"].is_null() || !input["forked_from_message_id"].is_null() {
            return Err(device_invalid());
        }
        let digest = hash(json_string(&input)?.as_bytes());
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Write)
            .await?;
        if let Some(ref key) = key {
            if key.is_empty() || key.len() > 200 {
                return Err(device_invalid());
            }
            if let Some(row)=sqlx::query("SELECT payload,request_sha256,project_id,deleted FROM android_personal_threads WHERE owner_id=$1 AND creation_key=$2").bind(actor.identity_id().to_string()).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)?{if row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=digest || row.try_get::<String,_>("project_id").map_err(storage_error)?!=project.to_string() || row.try_get::<i64,_>("deleted").map_err(storage_error)?!=0{return Err(Error::Conflict)}let v=parse(&row.try_get::<String,_>("payload").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(v)}
        }
        let id = Uuid::new_v4();
        let v = json!({"schema_version":2,"id":id,"revision":1,"etag":format!("{id}:1"),"project_id":project,"forked_from_thread_id":null,"forked_from_message_id":null,"title":title,"deleted_at":null,"created_at":device_iso(current)?});
        crate::android_wire_sync::record(
            &mut tx,
            actor.identity_id(),
            "thread",
            id,
            None,
            Some(project),
            1,
            &v,
            false,
            current,
        )
        .await?;
        sqlx::query("INSERT INTO android_personal_threads(id,project_id,owner_id,revision,payload,deleted,creation_key,request_sha256) VALUES($1,$2,$3,1,$4,0,$5,$6)").bind(id.to_string()).bind(project.to_string()).bind(actor.identity_id().to_string()).bind(json_string(&v)?).bind(key).bind(digest).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_personal_update_thread(
        &self,
        actor: &Principal,
        id: Uuid,
        input: Value,
        delete: bool,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let row = sqlx::query(
            "SELECT project_id,payload,deleted,owner_id FROM android_personal_threads WHERE id=$1",
        )
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        if row
            .try_get::<String, _>("owner_id")
            .map_err(storage_error)?
            != actor.identity_id().to_string()
            || row.try_get::<i64, _>("deleted").map_err(storage_error)? != 0
        {
            return Err(Error::NotFound);
        }
        let mut v: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        self.android_personal_permit(
            &mut tx,
            actor,
            wire_uuid(&v, "project_id")?,
            ContentAction::Write,
        )
        .await?;
        let old = revision(&input, "expected_revision")?;
        if revision(&v, "revision")? != old {
            return Err(Error::Conflict);
        }
        if delete {
            crate::android_wire_materialization::delete_messages(
                &mut tx,
                actor.identity_id(),
                id,
                current,
            )
            .await?;
            v["deleted_at"] = json!(device_iso(current)?);
        } else {
            v["title"] = json!(text(&input, "title", 200, false)?);
        }
        bump(&mut v, old + 1, current)?;
        let n=sqlx::query("UPDATE android_personal_threads SET revision=$1,payload=$2,deleted=$3 WHERE id=$4 AND revision=$5").bind(old+1).bind(json_string(&v)?).bind(i64::from(delete)).bind(id.to_string()).bind(old).execute(&mut *tx).await.map_err(storage_error)?;
        if n.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        crate::android_wire_sync::record(
            &mut tx,
            actor.identity_id(),
            "thread",
            id,
            None,
            Some(wire_uuid(&v, "project_id")?),
            old + 1,
            &v,
            delete,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_personal_operation(
        &self,
        actor: &Principal,
        project: Uuid,
        key: &str,
        digest: &str,
        receipt: Option<&Value>,
    ) -> Result<Option<Value>> {
        if key.is_empty() || key.len() > 240 || digest.len() != 64 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        if let Some(row)=sqlx::query("SELECT request_sha256,receipt,project_id FROM android_personal_operations WHERE owner_id=$1 AND operation_key=$2").bind(actor.identity_id().to_string()).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)?{if row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=digest||row.try_get::<String,_>("project_id").map_err(storage_error)?!=project.to_string(){return Err(Error::Conflict)}let value=parse(&row.try_get::<String,_>("receipt").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(Some(value))}
        if let Some(value) = receipt {
            sqlx::query("INSERT INTO android_personal_operations(owner_id,operation_key,project_id,request_sha256,receipt) VALUES($1,$2,$3,$4,$5)").bind(actor.identity_id().to_string()).bind(key).bind(project.to_string()).bind(digest).bind(json_string(value)?).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt.cloned())
    }
    pub(crate) async fn android_task_project_permit(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        project: Uuid,
        action: ContentAction,
    ) -> Result<()> {
        let personal: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_projects WHERE id=$1")
                .bind(project.to_string())
                .fetch_one(&mut **tx)
                .await
                .map_err(storage_error)?;
        if personal == 1 {
            self.android_personal_permit(tx, actor, project, action)
                .await?;
        } else {
            let team: String =
                sqlx::query_scalar("SELECT organization_id FROM tenant_projects WHERE id=$1")
                    .bind(project.to_string())
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(storage_error)?
                    .ok_or(Error::NotFound)?;
            let team = Uuid::parse_str(&team).map_err(storage_error)?;
            let permit = self
                .identity()
                .authorize_content_in_transaction(tx, actor, team, project, action)
                .await
                .map_err(identity_error)?;
            self.identity()
                .validate_content_in_transaction(tx, &permit)
                .await
                .map_err(identity_error)?;
            let archived:i64=sqlx::query_scalar("SELECT COALESCE((SELECT archived FROM team_project_profiles WHERE project_id=$1),0)").bind(project.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
            if archived != 0 {
                return Err(Error::Conflict);
            }
        }
        Ok(())
    }
    pub async fn android_tasks(&self, actor: &Principal, project: Uuid) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_task_project_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        let rows=sqlx::query("SELECT v.payload FROM android_task_versions v WHERE v.project_id=$1 AND v.revision=(SELECT MAX(w.revision) FROM android_task_versions w WHERE w.id=v.id) AND v.deleted=0 ORDER BY v.id LIMIT 2000").bind(project.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let v = rows
            .iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<Value>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_task_write(
        &self,
        actor: &Principal,
        id: Option<Uuid>,
        input: Value,
        mode: &str,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let old = if let Some(id) = id {
            let row=sqlx::query("SELECT payload FROM android_task_versions WHERE id=$1 ORDER BY revision DESC LIMIT 1").bind(id.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            Some(parse::<Value>(
                &row.try_get::<String, _>("payload").map_err(storage_error)?,
            )?)
        } else {
            None
        };
        let project = match &old {
            Some(v) => wire_uuid(v, "project_id")?,
            None => wire_uuid(&input, "project_id")?,
        };
        self.android_task_project_permit(&mut tx, actor, project, ContentAction::Write)
            .await?;
        let (id, rev) = if let Some(v) = &old {
            if !v["deleted_at"].is_null() {
                return Err(Error::NotFound);
            }
            let rev = revision(v, "revision")?;
            let expected = match mode {
                "version" => revision(&input, "base_revision")?,
                "release" => revision(&input, "revision")?,
                _ => revision(&input, "expected_revision")?,
            };
            if expected != rev {
                return Err(Error::Conflict);
            }
            (wire_uuid(v, "id")?, rev)
        } else {
            (Uuid::new_v4(), 0)
        };
        if mode == "release" {
            let mut v = old.ok_or_else(device_invalid)?;
            v["released"] = json!(true);
            sqlx::query("UPDATE android_task_versions SET released=1,payload=$1 WHERE id=$2 AND revision=$3 AND deleted=0").bind(json_string(&v)?).bind(id.to_string()).bind(rev).execute(&mut *tx).await.map_err(storage_error)?;
            let task_team: Option<String> =
                sqlx::query_scalar("SELECT organization_id FROM tenant_projects WHERE id=$1")
                    .bind(project.to_string())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            let task_team = task_team
                .map(|s| Uuid::parse_str(&s).map_err(storage_error))
                .transpose()?;
            crate::android_wire_sync::record(
                &mut tx,
                actor.identity_id(),
                "task",
                id,
                task_team,
                Some(project),
                v["revision"].as_i64().ok_or_else(device_invalid)?,
                &v,
                !v["deleted_at"].is_null(),
                current,
            )
            .await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        if mode == "delete" {
            let mut v = old.ok_or_else(device_invalid)?;
            v["revision"] = json!(rev + 1);
            v["etag"] = json!(format!("{id}:{}", rev + 1));
            v["deleted_at"] = json!(device_iso(current)?);
            sqlx::query("UPDATE android_task_versions SET deleted=1 WHERE id=$1")
                .bind(id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
            let task_team: Option<String> =
                sqlx::query_scalar("SELECT organization_id FROM tenant_projects WHERE id=$1")
                    .bind(project.to_string())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            let task_team = task_team
                .map(|s| Uuid::parse_str(&s).map_err(storage_error))
                .transpose()?;
            crate::android_wire_sync::record(
                &mut tx,
                actor.identity_id(),
                "task",
                id,
                task_team,
                Some(project),
                v["revision"].as_i64().ok_or_else(device_invalid)?,
                &v,
                !v["deleted_at"].is_null(),
                current,
            )
            .await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let name = text(&input, "name", 200, false)?;
        let instructions = text(&input, "instructions", 128 * 1024, false)?;
        let caps = input["required_capabilities"]
            .as_array()
            .ok_or_else(device_invalid)?;
        if caps.len() > 64
            || caps
                .iter()
                .any(|c| c.as_str().is_none_or(|c| c.is_empty() || c.len() > 100))
        {
            return Err(device_invalid());
        }
        if json_string(&input["config"])?.len() > 128 * 1024 {
            return Err(device_invalid());
        }
        let release = input["release"].as_bool().ok_or_else(device_invalid)?;
        let v = json!({"schema_version":2,"id":id,"revision":rev+1,"etag":format!("{id}:{}",rev+1),"project_id":project,"name":name,"instructions":instructions,"required_capabilities":caps,"default_target":input["default_target"],"config":input["config"],"released":release,"created_at":device_iso(current)?,"deleted_at":null});
        sqlx::query("INSERT INTO android_task_versions(id,revision,project_id,payload,released,deleted) VALUES($1,$2,$3,$4,$5,0)").bind(id.to_string()).bind(rev+1).bind(project.to_string()).bind(json_string(&v)?).bind(i64::from(release)).execute(&mut *tx).await.map_err(storage_error)?;
        let task_team: Option<String> =
            sqlx::query_scalar("SELECT organization_id FROM tenant_projects WHERE id=$1")
                .bind(project.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        let task_team = task_team
            .map(|s| Uuid::parse_str(&s).map_err(storage_error))
            .transpose()?;
        crate::android_wire_sync::record(
            &mut tx,
            actor.identity_id(),
            "task",
            id,
            task_team,
            Some(project),
            v["revision"].as_i64().ok_or_else(device_invalid)?,
            &v,
            !v["deleted_at"].is_null(),
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_task_frozen(
        &self,
        actor: &Principal,
        project: Uuid,
        reference: &Value,
    ) -> Result<Value> {
        let id = wire_uuid(reference, "id")?;
        let rev = revision(reference, "revision")?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_task_project_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        let payload:String=sqlx::query_scalar("SELECT payload FROM android_task_versions WHERE id=$1 AND revision=$2 AND project_id=$3 AND released=1 AND deleted=0").bind(id.to_string()).bind(rev).bind(project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let v = parse(&payload)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_personal_runs(
        &self,
        actor: &Principal,
        project: Uuid,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        let rows=sqlx::query("SELECT payload,created_at FROM android_scoped_runs WHERE team_id IS NULL AND creator_id=$1 AND project_id=$2 UNION ALL SELECT payload,created_at FROM android_private_server_runs WHERE owner_id=$1 AND project_id=$2 ORDER BY created_at LIMIT 2000").bind(actor.identity_id().to_string()).bind(project.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let values = rows
            .iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<Value>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_team_run_task(
        &self,
        actor: &Principal,
        run: Uuid,
        reference: Option<&Value>,
        input: Option<&Value>,
    ) -> Result<Option<(Value, Value)>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let row = sqlx::query("SELECT creator_id,project_id FROM team_runs WHERE id=$1")
            .bind(run.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        let project = Uuid::parse_str(
            &row.try_get::<String, _>("project_id")
                .map_err(storage_error)?,
        )
        .map_err(storage_error)?;
        self.android_task_project_permit(
            &mut tx,
            actor,
            project,
            if reference.is_some() {
                ContentAction::Execute
            } else {
                ContentAction::Read
            },
        )
        .await?;
        let old: Option<String> =
            sqlx::query_scalar("SELECT payload FROM android_team_run_tasks WHERE run_id=$1")
                .bind(run.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        if let Some(reference) = reference {
            if row
                .try_get::<String, _>("creator_id")
                .map_err(storage_error)?
                != actor.identity_id().to_string()
            {
                return Err(Error::Forbidden);
            }
            let id = wire_uuid(reference, "id")?;
            let revision = revision(reference, "revision")?;
            let payload =
                json_string(&json!({"task":reference,"input":input.ok_or_else(device_invalid)?}))?;
            if old.as_ref().is_some_and(|old| *old != payload) {
                return Err(Error::Conflict);
            }
            let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_task_versions WHERE id=$1 AND revision=$2 AND project_id=$3 AND released=1 AND deleted=0").bind(id.to_string()).bind(revision).bind(project.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if valid != 1 {
                return Err(Error::Conflict);
            }
            sqlx::query("INSERT INTO android_team_run_tasks(run_id,task_id,task_revision,payload) VALUES($1,$2,$3,$4) ON CONFLICT(run_id) DO NOTHING").bind(run.to_string()).bind(id.to_string()).bind(revision).bind(payload).execute(&mut *tx).await.map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(Some((
                reference.clone(),
                input.ok_or_else(device_invalid)?.clone(),
            )));
        }
        let value = old
            .map(|s| parse::<Value>(&s).map(|v| (v["task"].clone(), v["input"].clone())))
            .transpose()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}
