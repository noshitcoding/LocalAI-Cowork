//! V1 metadata replication with immutable operation receipts and current tenant visibility.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use cowork_identity::{ContentAction, Principal};
use serde::{Deserialize, Serialize};
use sqlx::{Any, Transaction, any::AnyRow};
use team_service::identity_error;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidWireSyncChange {
    pub schema_version: i64,
    pub operation_id: Uuid,
    pub device_id: Uuid,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub base_revision: i64,
    pub operation: String,
    pub payload: Option<Value>,
    pub client_timestamp: String,
}
pub(crate) async fn record(
    tx: &mut Transaction<'_, Any>,
    actor: Uuid,
    kind: &str,
    id: Uuid,
    team: Option<Uuid>,
    project: Option<Uuid>,
    revision: i64,
    payload: &Value,
    tombstone: bool,
    current: i64,
) -> Result<Value> {
    let mut merged = payload.clone();
    // Canonical API updates retain the legacy local shape and project membership list.
    if payload.get("schema_version") == Some(&json!(2)) && payload.get("id") == Some(&json!(id)) {
        let tracked:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_wire_projection_sources WHERE owner_id=$1 AND entity_type=$2 AND entity_id=$3").bind(actor.to_string()).bind(kind).bind(id.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
        if tracked == 1 {
            let previous:Option<String>=sqlx::query_scalar("SELECT payload FROM android_wire_sync_current WHERE owner_id=$1 AND entity_type=$2 AND entity_id=$3").bind(actor.to_string()).bind(kind).bind(id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
            if let Some(previous) = previous {
                let previous: Value = parse(&previous)?;
                if let Some(mut original) = previous["payload"].as_object().cloned() {
                    if let Some(update) = payload.as_object() {
                        original.extend(update.clone());
                    }
                    if kind == "project" {
                        original.insert("title".into(), payload["name"].clone());
                        original.insert("instructions".into(), payload["description"].clone());
                    }
                    if kind == "task" {
                        original.insert("title".into(), payload["name"].clone());
                        original.insert("description".into(), payload["instructions"].clone());
                        if let Some(metadata) = payload["config"]["sync_metadata"].as_object() {
                            original.extend(metadata.clone());
                        }
                    }
                    merged = Value::Object(original);
                }
            }
        }
    }
    let payload = &merged;
    let encoded = json_string(payload)?;
    let digest = hash(json_string(&(revision, payload, tombstone, team, project))?.as_bytes());
    let old:Option<String>=sqlx::query_scalar("SELECT digest FROM android_wire_sync_current WHERE owner_id=$1 AND entity_type=$2 AND entity_id=$3").bind(actor.to_string()).bind(kind).bind(id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let entity = json!({"schema_version":2,"entity_type":kind,"entity_id":id,"revision":revision,"etag":digest,"payload":if tombstone{Value::Null}else{payload.clone()},"tombstone":tombstone,"updated_at":device_iso(current)?});
    if old.as_deref() == Some(&digest) {
        return Ok(entity);
    }
    let cursor: i64 = sqlx::query_scalar(
        "UPDATE android_wire_sync_clock SET sequence=sequence+1 WHERE id=1 RETURNING sequence",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    sqlx::query("INSERT INTO android_wire_sync_changes(cursor,owner_id,entity_type,entity_id,team_id,project_id,revision,payload,tombstone,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)").bind(cursor).bind(actor.to_string()).bind(kind).bind(id.to_string()).bind(team.map(|v|v.to_string())).bind(project.map(|v|v.to_string())).bind(revision).bind(json_string(&entity)?).bind(i64::from(tombstone)).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    sqlx::query("INSERT INTO android_wire_sync_current(owner_id,entity_type,entity_id,team_id,project_id,revision,cursor,digest,payload,tombstone) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(owner_id,entity_type,entity_id) DO UPDATE SET team_id=excluded.team_id,project_id=excluded.project_id,revision=excluded.revision,cursor=excluded.cursor,digest=excluded.digest,payload=excluded.payload,tombstone=excluded.tombstone").bind(actor.to_string()).bind(kind).bind(id.to_string()).bind(team.map(|v|v.to_string())).bind(project.map(|v|v.to_string())).bind(revision).bind(cursor).bind(&digest).bind(json_string(&entity)?).bind(i64::from(tombstone)).execute(&mut **tx).await.map_err(storage_error)?;
    let _ = encoded;
    Ok(entity)
}
fn kind(kind: &str) -> Result<()> {
    if [
        "project",
        "thread",
        "message",
        "task",
        "schedule",
        "run",
        "crew",
        "skill",
        "memory",
        "provider_profile",
        "secret_metadata",
        "mcp_metadata",
    ]
    .contains(&kind)
    {
        Ok(())
    } else {
        Err(device_invalid())
    }
}
fn secrets(value: &Value) -> bool {
    match value {
        Value::Object(o) => o.iter().any(|(k, v)| {
            matches!(
                k.to_ascii_lowercase().as_str(),
                "api_key"
                    | "apikey"
                    | "access_token"
                    | "refresh_token"
                    | "password"
                    | "secret"
                    | "client_secret"
                    | "authorization"
            ) || secrets(v)
        }),
        Value::Array(a) => a.iter().any(secrets),
        _ => false,
    }
}
fn entity(row: &AnyRow) -> Result<Value> {
    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)
}
fn visible(query: &mut sqlx::QueryBuilder<'_, Any>, actor: Uuid, scope: Option<Uuid>) {
    query.push(" WHERE c.owner_id=").push_bind(actor.to_string()).push(" AND ((c.team_id IS NULL AND ").push_bind(scope.map(|v|v.to_string())).push(" IS NULL) OR (c.team_id IS NOT NULL AND (").push_bind(scope.map(|v|v.to_string())).push(" IS NULL OR c.team_id=").push_bind(scope.map(|v|v.to_string())).push(") AND EXISTS(SELECT 1 FROM identity_memberships m JOIN identity_organizations o ON o.id=m.organization_id WHERE m.identity_id=c.owner_id AND m.organization_id=c.team_id AND m.active=1 AND o.state='active' AND (c.project_id IS NULL OR EXISTS(SELECT 1 FROM identity_resources p JOIN identity_content_grants g ON g.resource_id=p.id AND g.membership_id=m.id WHERE p.id=c.project_id AND p.organization_id=o.id AND g.active=1 AND (g.permissions & 1)=1)))))");
}
impl Store {
    /// A retry uses the original server-frozen metadata. It cannot choose a new version under the old operation key.
    pub async fn android_personal_run_intent(
        &self,
        actor: &Principal,
        project: Uuid,
        key: &str,
        digest: &str,
        frozen: Option<&Value>,
    ) -> Result<Option<Value>> {
        if key.is_empty() || key.len() > 200 || digest.len() != 64 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        if let Some(row)=sqlx::query("SELECT project_id,request_sha256,frozen FROM android_personal_run_intents WHERE owner_id=$1 AND creation_key=$2").bind(actor.identity_id().to_string()).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)?{if row.try_get::<String,_>("project_id").map_err(storage_error)?!=project.to_string()||row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=digest{return Err(Error::Conflict)}let value=parse(&row.try_get::<String,_>("frozen").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(Some(value))}
        if let Some(value) = frozen {
            if json_string(value)?.len() > 1_000_000 {
                return Err(device_invalid());
            }
            sqlx::query("INSERT INTO android_personal_run_intents(owner_id,creation_key,project_id,request_sha256,frozen) VALUES($1,$2,$3,$4,$5)").bind(actor.identity_id().to_string()).bind(key).bind(project.to_string()).bind(digest).bind(json_string(value)?).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(frozen.cloned())
    }
    pub async fn android_wire_runtime_metadata(
        &self,
        actor: &Principal,
        project: Uuid,
        kind_name: &str,
        ids: &[Uuid],
    ) -> Result<Vec<Value>> {
        if !matches!(kind_name, "skill" | "memory" | "mcp_metadata") || ids.len() > 100 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        let allowed = match kind_name {
            "skill" => &[
                "name",
                "description",
                "prompt_template",
                "trigger_pattern",
                "run_mode",
                "auto_generated",
                "parent_skill_id",
                "source_task_ids",
            ][..],
            "memory" => &[
                "scope",
                "scope_ref",
                "category",
                "key",
                "content",
                "target",
                "source_run_id",
                "confidence",
                "source",
            ][..],
            _ => &[
                "name",
                "transport",
                "executable_hint",
                "environment_keys",
                "device_binding_required",
                "source",
            ][..],
        };
        let mut seen = std::collections::BTreeSet::new();
        let mut result = Vec::new();
        for id in ids {
            if !seen.insert(*id) {
                return Err(device_invalid());
            }
            let row=sqlx::query("SELECT payload,project_id,revision FROM android_wire_sync_current WHERE owner_id=$1 AND entity_type=$2 AND entity_id=$3 AND team_id IS NULL AND tombstone=0").bind(actor.identity_id().to_string()).bind(kind_name).bind(id.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            if let Some(project) = row
                .try_get::<Option<String>, _>("project_id")
                .map_err(storage_error)?
            {
                self.android_personal_permit(
                    &mut tx,
                    actor,
                    Uuid::parse_str(&project).map_err(storage_error)?,
                    ContentAction::Read,
                )
                .await?;
            }
            let stored = entity(&row)?;
            let payload = stored["payload"].as_object().ok_or_else(device_invalid)?;
            let definition = Value::Object(
                payload
                    .iter()
                    .filter(|(name, _)| allowed.contains(&name.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            );
            let required = match kind_name {
                "skill" => &["name", "prompt_template"][..],
                "memory" => &["key", "content"][..],
                _ => &["name"][..],
            };
            if required.iter().any(|name| {
                definition[*name]
                    .as_str()
                    .is_none_or(|v| v.trim().is_empty())
            }) {
                return Err(device_invalid());
            }
            result.push(json!({"entity_id":id,"revision":stored["revision"],"etag":stored["etag"],"definition":definition}));
        }
        if json_string(&result)?.len() > 128 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_wire_bind_device(&self, actor: &Principal, device: Uuid) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let old:Option<String>=sqlx::query_scalar("SELECT device_id FROM android_wire_session_devices WHERE session_id=$1 AND owner_id=$2").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if old.as_ref().is_some_and(|d| *d != device.to_string()) {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO android_wire_session_devices(session_id,owner_id,device_id) VALUES($1,$2,$3) ON CONFLICT(session_id) DO NOTHING").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(device.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_wire_sync_push(
        &self,
        actor: &Principal,
        changes: Vec<AndroidWireSyncChange>,
    ) -> Result<Value> {
        if changes.is_empty() || changes.len() > 100 {
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
        let device:String=sqlx::query_scalar("SELECT device_id FROM android_wire_session_devices WHERE session_id=$1 AND owner_id=$2").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let mut results = Vec::new();
        for change in changes {
            kind(&change.entity_type)?;
            if change.schema_version != 2
                || change.device_id.to_string() != device
                || change.base_revision < 0
                || !matches!(
                    change.entity_type.as_str(),
                    "project"
                        | "thread"
                        | "task"
                        | "message"
                        | "crew"
                        | "skill"
                        | "memory"
                        | "secret_metadata"
                        | "mcp_metadata"
                )
                || !matches!(change.operation.as_str(), "upsert" | "delete")
            {
                return Err(device_invalid());
            }
            let at = chrono::DateTime::parse_from_rfc3339(&change.client_timestamp)
                .map_err(|_| device_invalid())?
                .timestamp_millis();
            if at > current + 300_000 {
                return Err(device_invalid());
            }
            if (change.operation == "upsert") != change.payload.is_some()
                || change.payload.as_ref().is_some_and(|v| {
                    json_string(v).map_or(true, |s| s.len() > 512 * 1024) || secrets(v)
                })
            {
                return Err(device_invalid());
            }
            if let Some(payload) = &change.payload {
                if !payload.is_object()
                    || payload.get("team_id").is_some_and(|v| !v.is_null())
                    || payload
                        .get("owner_user_id")
                        .filter(|v| !v.is_null())
                        .is_some_and(|v| {
                            v.as_str() != Some(actor.identity_id().to_string().as_str())
                        })
                {
                    return Err(device_invalid());
                }
                if !matches!(
                    change.entity_type.as_str(),
                    "project" | "thread" | "task" | "message"
                ) {
                    if let Some(id) = payload.get("project_id").filter(|v| !v.is_null()) {
                        let project = id
                            .as_str()
                            .and_then(|s| Uuid::parse_str(s).ok())
                            .ok_or_else(device_invalid)?;
                        self.android_personal_permit(&mut tx, actor, project, ContentAction::Write)
                            .await?;
                    }
                }
            }
            let digest = hash(json_string(&change)?.as_bytes());
            if let Some(row)=sqlx::query("SELECT request_sha256,receipt FROM android_wire_sync_receipts WHERE owner_id=$1 AND operation_id=$2").bind(actor.identity_id().to_string()).bind(change.operation_id.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?{if row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=digest{return Err(Error::Conflict)}results.push(parse::<Value>(&row.try_get::<String,_>("receipt").map_err(storage_error)?)?);continue;}
            let old=sqlx::query("SELECT payload,revision,team_id FROM android_wire_sync_current WHERE owner_id=$1 AND entity_type=$2 AND entity_id=$3").bind(actor.identity_id().to_string()).bind(&change.entity_type).bind(change.entity_id.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
            if old.as_ref().is_some_and(|r| {
                r.try_get::<Option<String>, _>("team_id")
                    .ok()
                    .flatten()
                    .is_some()
            }) {
                return Err(Error::Forbidden);
            }
            let revision = old
                .as_ref()
                .map(|r| r.try_get::<i64, _>("revision").map_err(storage_error))
                .transpose()?
                .unwrap_or(0);
            let result = if revision != change.base_revision {
                json!({"schema_version":2,"operation_id":change.operation_id,"status":"conflict","entity":old.as_ref().map(entity).transpose()?})
            } else {
                crate::android_wire_materialization::admit(
                    &mut tx,
                    actor.identity_id(),
                    &change.entity_type,
                    change.entity_id,
                )
                .await?;
                let payload = change.payload.unwrap_or(Value::Null);
                let project = payload
                    .get("project_id")
                    .and_then(Value::as_str)
                    .and_then(|s| Uuid::parse_str(s).ok());
                let value = record(
                    &mut tx,
                    actor.identity_id(),
                    &change.entity_type,
                    change.entity_id,
                    None,
                    project,
                    revision + 1,
                    &payload,
                    change.operation == "delete",
                    current,
                )
                .await?;
                json!({"schema_version":2,"operation_id":change.operation_id,"status":"applied","entity":value})
            };
            sqlx::query("INSERT INTO android_wire_sync_receipts(owner_id,operation_id,request_sha256,receipt) VALUES($1,$2,$3,$4)").bind(actor.identity_id().to_string()).bind(change.operation_id.to_string()).bind(digest).bind(json_string(&result)?).execute(&mut *tx).await.map_err(storage_error)?;
            results.push(result);
        }
        crate::android_wire_materialization::reconcile(self, &mut tx, actor, current).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"schema_version":2,"results":results}))
    }
    pub async fn android_wire_sync_capture(
        &self,
        actor: &Principal,
        kind_name: &str,
        id: Uuid,
        team: Option<Uuid>,
        project: Option<Uuid>,
        revision: i64,
        payload: Value,
        tombstone: bool,
    ) -> Result<()> {
        kind(kind_name)?;
        if revision <= 0 || json_string(&payload)?.len() > 512 * 1024 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        if let Some(team) = team {
            if let Some(project) = project {
                let p = self
                    .identity()
                    .authorize_content_in_transaction(
                        &mut tx,
                        actor,
                        team,
                        project,
                        ContentAction::Read,
                    )
                    .await
                    .map_err(identity_error)?;
                self.identity()
                    .validate_content_in_transaction(&mut tx, &p)
                    .await
                    .map_err(identity_error)?;
            } else {
                self.identity()
                    .organization_in_transaction(&mut tx, actor, team)
                    .await
                    .map_err(identity_error)?;
            }
        } else {
            self.identity()
                .require_global_session_in_transaction(&mut tx, actor)
                .await
                .map_err(identity_error)?;
        }
        if team.is_none() {
            let tracked:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_wire_projection_sources WHERE owner_id=$1 AND entity_type=$2 AND entity_id=$3").bind(actor.identity_id().to_string()).bind(kind_name).bind(id.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if tracked == 1 {
                tx.commit().await.map_err(storage_error)?;
                return Ok(());
            }
        }
        let old=sqlx::query("SELECT team_id,project_id,revision FROM android_wire_sync_current WHERE owner_id=$1 AND entity_type=$2 AND entity_id=$3").bind(actor.identity_id().to_string()).bind(kind_name).bind(id.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if let Some(row) = old {
            if row
                .try_get::<Option<String>, _>("team_id")
                .map_err(storage_error)?
                != team.map(|v| v.to_string())
            {
                return Err(Error::Conflict);
            }
            if row.try_get::<i64, _>("revision").map_err(storage_error)? > revision {
                tx.commit().await.map_err(storage_error)?;
                return Ok(());
            }
        }
        record(
            &mut tx,
            actor.identity_id(),
            kind_name,
            id,
            team,
            project,
            revision,
            &payload,
            tombstone,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_wire_sync_pull(
        &self,
        actor: &Principal,
        after: i64,
        limit: i64,
    ) -> Result<Value> {
        if after < 0 || !(1..=1000).contains(&limit) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let scope = self
            .identity()
            .session_organization_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let mut query =
            sqlx::QueryBuilder::<Any>::new("SELECT c.* FROM android_wire_sync_changes c");
        visible(&mut query, actor.identity_id(), scope);
        query
            .push(" AND c.cursor>")
            .push_bind(after)
            .push(" ORDER BY c.cursor LIMIT ")
            .push_bind(limit);
        let rows = query
            .build()
            .fetch_all(&mut *tx)
            .await
            .map_err(storage_error)?;
        let mut values = Vec::new();
        let mut next = after;
        for row in rows {
            next = row.try_get("cursor").map_err(storage_error)?;
            let v = entity(&row)?;
            values.push(json!({"schema_version":2,"cursor":next,"entity_type":v["entity_type"],"entity_id":v["entity_id"],"revision":v["revision"],"operation":if v["tombstone"]==true{"delete"}else{"upsert"},"payload":v["payload"],"created_at":v["updated_at"]}));
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"schema_version":2,"changes":values,"next_cursor":next}))
    }
    pub async fn android_wire_sync_frame(
        &self,
        actor: &Principal,
        cursor: i64,
    ) -> Result<Option<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let scope = self
            .identity()
            .session_organization_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let mut query =
            sqlx::QueryBuilder::<Any>::new("SELECT c.* FROM android_wire_sync_changes c");
        visible(&mut query, actor.identity_id(), scope);
        query.push(" AND c.cursor=").push_bind(cursor);
        let row = query
            .build()
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let value=row.map(|row|entity(&row).map(|v|json!({"schema_version":2,"cursor":cursor,"entity_type":v["entity_type"],"entity_id":v["entity_id"],"revision":v["revision"],"operation":if v["tombstone"]==true{"delete"}else{"upsert"},"payload":v["payload"],"created_at":v["updated_at"]}))).transpose()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_wire_sync_entities(
        &self,
        actor: &Principal,
        kind_name: &str,
        after: Option<Uuid>,
        cursor: Option<i64>,
        limit: i64,
    ) -> Result<Value> {
        kind(kind_name)?;
        if !(1..=1000).contains(&limit) || cursor.is_some_and(|v| v < 0) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let scope = self
            .identity()
            .session_organization_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let mut query =
            sqlx::QueryBuilder::<Any>::new("SELECT c.* FROM android_wire_sync_current c");
        visible(&mut query, actor.identity_id(), scope);
        query
            .push(" AND c.entity_type=")
            .push_bind(kind_name.to_owned());
        if let Some(cursor) = cursor {
            query
                .push(" AND c.cursor>")
                .push_bind(cursor)
                .push(" ORDER BY c.cursor");
        } else {
            query
                .push(" AND c.entity_id>")
                .push_bind(after.map(|v| v.to_string()).unwrap_or_default())
                .push(" ORDER BY c.entity_id");
        }
        query.push(" LIMIT ").push_bind(limit + 1);
        let rows = query
            .build()
            .fetch_all(&mut *tx)
            .await
            .map_err(storage_error)?;
        let more = rows.len() as i64 > limit;
        let mut values = Vec::new();
        let mut next = cursor.unwrap_or(0);
        for row in rows.into_iter().take(limit as usize) {
            next = row.try_get("cursor").map_err(storage_error)?;
            values.push(entity(&row)?);
        }
        let watermark: i64 =
            sqlx::query_scalar("SELECT sequence FROM android_wire_sync_clock WHERE id=1")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"schema_version":2,"next_after":if more{values.last().map(|v|v["entity_id"].clone())}else{None},"items":values,"watermark_cursor":watermark,"next_cursor":if more{next}else{watermark},"has_more":more}),
        )
    }
}
