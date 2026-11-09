//! Project accepted local metadata into real personal V1 content. Imported messages are inert history.
//! No provider, tool, schedule or execution is dispatched by materialization.
use super::*;
use crate::android_devices::{device_invalid, device_iso};
use crate::android_personal::wire_uuid;
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction};

fn text(v: &Value, names: &[&str], max: usize, empty: bool) -> Result<String> {
    let value = names
        .iter()
        .find_map(|name| v.get(*name).and_then(Value::as_str))
        .unwrap_or("");
    if value.len() > max || value.contains('\0') || (!empty && value.trim().is_empty()) {
        return Err(device_invalid());
    }
    Ok(value.to_owned())
}
fn optional_id(v: &Value, name: &str) -> Result<Option<Uuid>> {
    v.get(name)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .and_then(|v| Uuid::parse_str(v).ok())
                .ok_or_else(device_invalid)
        })
        .transpose()
}
fn target(v: &Value) -> Result<()> {
    if v.is_null() {
        return Ok(());
    }
    let o = v.as_object().ok_or_else(device_invalid)?;
    match v["kind"].as_str() {
        Some("personal_device") if o.len() == 2 && optional_id(v, "device_id")?.is_some() => Ok(()),
        Some("server_linux") if o.len() <= 2 && v.get("pool_id").is_none_or(Value::is_null) => {
            Ok(())
        }
        Some("managed_windows_pool") if o.len() == 2 && optional_id(v, "pool_id")?.is_some() => {
            Ok(())
        }
        _ => Err(device_invalid()),
    }
}
async fn tracked(tx: &mut Transaction<'_, Any>, actor: Uuid, kind: &str, id: Uuid) -> Result<bool> {
    let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_wire_projection_sources WHERE owner_id=$1 AND entity_type=$2 AND entity_id=$3").bind(actor.to_string()).bind(kind).bind(id.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    Ok(n == 1)
}
/// Independent server objects cannot be adopted by an unrelated local sync ID.
pub(crate) async fn admit(
    tx: &mut Transaction<'_, Any>,
    actor: Uuid,
    kind: &str,
    id: Uuid,
) -> Result<()> {
    if !matches!(kind, "project" | "thread" | "task" | "message") {
        return Ok(());
    }
    let sql = match kind {
        "project" => "SELECT COUNT(*) FROM android_personal_projects WHERE id=$1",
        "thread" => "SELECT COUNT(*) FROM android_personal_threads WHERE id=$1",
        "task" => "SELECT COUNT(*) FROM android_task_versions WHERE id=$1",
        _ => "SELECT COUNT(*) FROM android_personal_messages WHERE id=$1",
    };
    let n: i64 = sqlx::query_scalar(sql)
        .bind(id.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let foreign:i64=match kind {
        "project"=>sqlx::query_scalar("SELECT COUNT(*) FROM tenant_projects WHERE id=$1").bind(id.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?,
        "thread"=>sqlx::query_scalar("SELECT COUNT(*) FROM team_orders WHERE id=$1").bind(id.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?,
        "message"=>sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM android_scoped_runs WHERE id=$1 OR id=$2)+(SELECT COUNT(*) FROM team_runs WHERE id=$1 OR id=$2)").bind(id.to_string()).bind(Uuid::from_u128(id.as_u128()^1).to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?,
        _=>0,
    };
    let owner:Option<String>=sqlx::query_scalar("SELECT owner_id FROM android_wire_projection_sources WHERE entity_type=$1 AND entity_id=$2").bind(kind).bind(id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    if foreign > 0
        || n > 0 && !tracked(tx, actor, kind, id).await?
        || owner.as_deref().is_some_and(|v| v != actor.to_string())
    {
        return Err(Error::Conflict);
    }
    sqlx::query("INSERT INTO android_wire_projection_sources(owner_id,entity_type,entity_id) VALUES($1,$2,$3) ON CONFLICT(owner_id,entity_type,entity_id) DO NOTHING").bind(actor.to_string()).bind(kind).bind(id.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
async fn project_memberships(
    tx: &mut Transaction<'_, Any>,
    actor: Uuid,
) -> Result<std::collections::BTreeMap<Uuid, Uuid>> {
    let rows=sqlx::query("SELECT c.entity_id,c.payload FROM android_wire_sync_current c JOIN android_wire_projection_sources p ON p.owner_id=c.owner_id AND p.entity_type=c.entity_type AND p.entity_id=c.entity_id WHERE c.owner_id=$1 AND c.entity_type='project' AND c.team_id IS NULL AND c.tombstone=0 ORDER BY c.entity_id LIMIT 10001").bind(actor.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 10000 {
        return Err(Error::BudgetExceeded);
    }
    let mut found = std::collections::BTreeMap::new();
    for row in rows {
        let entity: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if let Some(ids) = entity["payload"].get("thread_ids") {
            let ids = ids.as_array().ok_or_else(device_invalid)?;
            if ids.len() > 2000 {
                return Err(device_invalid());
            }
            for value in ids {
                let child = value
                    .as_str()
                    .and_then(|v| Uuid::parse_str(v).ok())
                    .ok_or_else(device_invalid)?;
                let parent = Uuid::parse_str(
                    &row.try_get::<String, _>("entity_id")
                        .map_err(storage_error)?,
                )
                .map_err(storage_error)?;
                if found.insert(child, parent).is_some_and(|old| old != parent) {
                    return Err(Error::Conflict);
                }
            }
        }
    }
    Ok(found)
}
async fn private_parent(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    project: Uuid,
) -> Result<bool> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM android_personal_projects WHERE id=$1 AND owner_id=$2 AND deleted=0",
    )
    .bind(project.to_string())
    .bind(actor.identity_id().to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if n == 0 {
        return Ok(false);
    }
    store
        .android_personal_permit(tx, actor, project, ContentAction::Write)
        .await?;
    Ok(true)
}
async fn mark(
    tx: &mut Transaction<'_, Any>,
    actor: Uuid,
    kind: &str,
    id: Uuid,
    digest: &str,
) -> Result<()> {
    sqlx::query("UPDATE android_wire_projection_sources SET applied_digest=$1 WHERE owner_id=$2 AND entity_type=$3 AND entity_id=$4").bind(digest).bind(actor.to_string()).bind(kind).bind(id.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
pub(crate) async fn delete_messages(
    tx: &mut Transaction<'_, Any>,
    actor: Uuid,
    thread: Uuid,
    current: i64,
) -> Result<()> {
    let rows=sqlx::query("SELECT id,project_id,revision,payload FROM android_personal_messages WHERE owner_id=$1 AND thread_id=$2 AND deleted=0").bind(actor.to_string()).bind(thread.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    for row in rows {
        let id = Uuid::parse_str(&row.try_get::<String, _>("id").map_err(storage_error)?)
            .map_err(storage_error)?;
        let project = Uuid::parse_str(
            &row.try_get::<String, _>("project_id")
                .map_err(storage_error)?,
        )
        .map_err(storage_error)?;
        let revision = row.try_get::<i64, _>("revision").map_err(storage_error)? + 1;
        let mut v: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        v["revision"] = json!(revision);
        v["deleted_at"] = json!(device_iso(current)?);
        v["updated_at"] = json!(device_iso(current)?);
        sqlx::query("UPDATE android_personal_messages SET revision=$1,payload=$2,deleted=1 WHERE id=$3 AND owner_id=$4").bind(revision).bind(json_string(&v)?).bind(id.to_string()).bind(actor.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
        crate::android_wire_sync::record(
            tx,
            actor,
            "message",
            id,
            None,
            Some(project),
            revision,
            &v,
            true,
            current,
        )
        .await?;
    }
    // Provider-produced messages are exposed from real run receipts rather than this table.
    // Their replicated copies must receive tombstones as well.
    let copies=sqlx::query("SELECT entity_id,project_id,revision,payload FROM android_wire_sync_current WHERE owner_id=$1 AND team_id IS NULL AND entity_type='message' AND tombstone=0 LIMIT 10001").bind(actor.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if copies.len() > 10000 {
        return Err(Error::BudgetExceeded);
    }
    for copy in copies {
        let entity: Value = parse(
            &copy
                .try_get::<String, _>("payload")
                .map_err(storage_error)?,
        )?;
        if entity["payload"]["thread_id"] != json!(thread) {
            continue;
        }
        let id = Uuid::parse_str(
            &copy
                .try_get::<String, _>("entity_id")
                .map_err(storage_error)?,
        )
        .map_err(storage_error)?;
        let project = copy
            .try_get::<Option<String>, _>("project_id")
            .map_err(storage_error)?
            .map(|s| Uuid::parse_str(&s).map_err(storage_error))
            .transpose()?;
        let revision = copy.try_get::<i64, _>("revision").map_err(storage_error)? + 1;
        crate::android_wire_sync::record(
            tx,
            actor,
            "message",
            id,
            None,
            project,
            revision,
            &entity["payload"],
            true,
            current,
        )
        .await?;
    }
    Ok(())
}
async fn delete_thread(
    tx: &mut Transaction<'_, Any>,
    actor: Uuid,
    id: Uuid,
    current: i64,
) -> Result<()> {
    let row=sqlx::query("SELECT project_id,revision,payload FROM android_personal_threads WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(id.to_string()).bind(actor.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    if let Some(row) = row {
        delete_messages(tx, actor, id, current).await?;
        let project = Uuid::parse_str(
            &row.try_get::<String, _>("project_id")
                .map_err(storage_error)?,
        )
        .map_err(storage_error)?;
        let revision = row.try_get::<i64, _>("revision").map_err(storage_error)? + 1;
        let mut v: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        v["revision"] = json!(revision);
        v["deleted_at"] = json!(device_iso(current)?);
        sqlx::query("UPDATE android_personal_threads SET revision=$1,payload=$2,deleted=1 WHERE id=$3 AND owner_id=$4").bind(revision).bind(json_string(&v)?).bind(id.to_string()).bind(actor.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
        crate::android_wire_sync::record(
            tx,
            actor,
            "thread",
            id,
            None,
            Some(project),
            revision,
            &v,
            true,
            current,
        )
        .await?;
    }
    Ok(())
}
pub(crate) async fn delete_project_children(
    tx: &mut Transaction<'_, Any>,
    actor: Uuid,
    project: Uuid,
    current: i64,
) -> Result<()> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM android_personal_threads WHERE owner_id=$1 AND project_id=$2 AND deleted=0",
    )
    .bind(actor.to_string())
    .bind(project.to_string())
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    for id in rows {
        delete_thread(
            tx,
            actor,
            Uuid::parse_str(&id).map_err(storage_error)?,
            current,
        )
        .await?;
    }
    let rows=sqlx::query("SELECT v.id,v.revision,v.payload FROM android_task_versions v WHERE v.project_id=$1 AND v.deleted=0 AND v.revision=(SELECT MAX(w.revision) FROM android_task_versions w WHERE w.id=v.id)").bind(project.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    for row in rows {
        let id = Uuid::parse_str(&row.try_get::<String, _>("id").map_err(storage_error)?)
            .map_err(storage_error)?;
        let revision = row.try_get::<i64, _>("revision").map_err(storage_error)? + 1;
        let mut v: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        v["revision"] = json!(revision);
        v["released"] = json!(false);
        v["deleted_at"] = json!(device_iso(current)?);
        sqlx::query("UPDATE android_task_versions SET deleted=1,released=0 WHERE id=$1")
            .bind(id.to_string())
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
        crate::android_wire_sync::record(
            tx,
            actor,
            "task",
            id,
            None,
            Some(project),
            revision,
            &v,
            true,
            current,
        )
        .await?;
    }
    Ok(())
}
/// Parent-first reconciliation also handles a legacy thread/message arriving before its project.
pub(crate) async fn reconcile(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    current: i64,
) -> Result<()> {
    let memberships = project_memberships(tx, actor.identity_id()).await?;
    for kind in ["project", "thread", "task", "message"] {
        let rows=sqlx::query("SELECT c.entity_id,c.payload,c.digest,c.revision,c.tombstone,p.applied_digest FROM android_wire_sync_current c JOIN android_wire_projection_sources p ON p.owner_id=c.owner_id AND p.entity_type=c.entity_type AND p.entity_id=c.entity_id WHERE c.owner_id=$1 AND c.entity_type=$2 AND c.team_id IS NULL ORDER BY c.entity_id LIMIT 10001").bind(actor.identity_id().to_string()).bind(kind).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if rows.len() > 10000 {
            return Err(Error::BudgetExceeded);
        }
        for row in rows {
            let id = Uuid::parse_str(
                &row.try_get::<String, _>("entity_id")
                    .map_err(storage_error)?,
            )
            .map_err(storage_error)?;
            let digest = row.try_get::<String, _>("digest").map_err(storage_error)?;
            if kind != "thread"
                && row
                    .try_get::<Option<String>, _>("applied_digest")
                    .map_err(storage_error)?
                    .as_deref()
                    == Some(&digest)
            {
                continue;
            }
            let revision = row.try_get::<i64, _>("revision").map_err(storage_error)?;
            let deleted = row.try_get::<i64, _>("tombstone").map_err(storage_error)? != 0;
            let entity: Value =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let payload = &entity["payload"];
            let timestamp = device_iso(current)?;
            match kind {
                "project" => {
                    let old = sqlx::query(
                        "SELECT owner_id,payload FROM android_personal_projects WHERE id=$1",
                    )
                    .bind(id.to_string())
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(storage_error)?;
                    if old.as_ref().is_some_and(|r| {
                        r.try_get::<String, _>("owner_id").ok().as_deref()
                            != Some(actor.identity_id().to_string().as_str())
                    }) {
                        return Err(Error::Forbidden);
                    }
                    if deleted {
                        delete_project_children(tx, actor.identity_id(), id, current).await?;
                        if let Some(old) = old {
                            let mut value: Value = parse(
                                &old.try_get::<String, _>("payload").map_err(storage_error)?,
                            )?;
                            value["revision"] = json!(revision);
                            value["deleted_at"] = json!(timestamp);
                            sqlx::query("UPDATE android_personal_projects SET revision=$1,payload=$2,deleted=1 WHERE id=$3").bind(revision).bind(json_string(&value)?).bind(id.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
                        }
                    } else {
                        if payload.get("privacy").is_some_and(|v| v != "private_local")
                            || payload.get("project_kind").is_some_and(|v| v != "private")
                        {
                            return Err(device_invalid());
                        }
                        let name = text(payload, &["title", "name"], 200, false)?;
                        let description =
                            text(payload, &["instructions", "description"], 200_000, true)?;
                        let policy = payload.get("policy").cloned().unwrap_or_else(|| json!({}));
                        if !policy.is_object() || json_string(&policy)?.len() > 64 * 1024 {
                            return Err(device_invalid());
                        }
                        target(&payload["preferred_executor_target"])?;
                        let created = old
                            .as_ref()
                            .map(|r| {
                                parse::<Value>(
                                    &r.try_get::<String, _>("payload").map_err(storage_error)?,
                                )
                            })
                            .transpose()?
                            .and_then(|v| v.get("created_at").cloned())
                            .unwrap_or_else(|| json!(timestamp));
                        let value = json!({"schema_version":2,"id":id,"revision":revision,"etag":format!("{id}:{revision}"),"owner_user_id":actor.identity_id(),"team_id":null,"privacy":"private_local","name":name,"description":description,"preferred_executor_target":payload["preferred_executor_target"],"current_version_id":null,"policy":policy,"created_at":created,"updated_at":timestamp,"deleted_at":null});
                        sqlx::query("INSERT INTO android_personal_projects(id,owner_id,revision,payload,deleted) VALUES($1,$2,$3,$4,0) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload,deleted=0").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(revision).bind(json_string(&value)?).execute(&mut **tx).await.map_err(storage_error)?;
                    }
                }
                "thread" => {
                    if deleted {
                        delete_thread(tx, actor.identity_id(), id, current).await?;
                    } else {
                        let explicit = optional_id(payload, "project_id")?;
                        let linked = memberships.get(&id).copied();
                        if explicit.is_some() && linked.is_some() && explicit != linked {
                            return Err(Error::Conflict);
                        }
                        let Some(project) = explicit.or(linked) else {
                            delete_thread(tx, actor.identity_id(), id, current).await?;
                            continue;
                        };
                        if !private_parent(store, tx, actor, project).await? {
                            continue;
                        }
                        let old:Option<String>=sqlx::query_scalar("SELECT payload FROM android_personal_threads WHERE id=$1 AND owner_id=$2").bind(id.to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
                        let created = old
                            .map(|s| parse::<Value>(&s))
                            .transpose()?
                            .and_then(|v| v.get("created_at").cloned())
                            .unwrap_or_else(|| json!(timestamp));
                        let value = json!({"schema_version":2,"id":id,"revision":revision,"etag":format!("{id}:{revision}"),"project_id":project,"forked_from_thread_id":null,"forked_from_message_id":null,"title":text(payload,&["title"],200,false)?,"created_at":created,"deleted_at":null});
                        sqlx::query("INSERT INTO android_personal_threads(id,project_id,owner_id,revision,payload,deleted) VALUES($1,$2,$3,$4,$5,0) ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id,revision=excluded.revision,payload=excluded.payload,deleted=0").bind(id.to_string()).bind(project.to_string()).bind(actor.identity_id().to_string()).bind(revision).bind(json_string(&value)?).execute(&mut **tx).await.map_err(storage_error)?;
                    }
                }
                "task" => {
                    if deleted {
                        sqlx::query(
                            "UPDATE android_task_versions SET deleted=1,released=0 WHERE id=$1",
                        )
                        .bind(id.to_string())
                        .execute(&mut **tx)
                        .await
                        .map_err(storage_error)?;
                    } else {
                        let explicit = optional_id(payload, "project_id")?;
                        let thread = optional_id(payload, "thread_id")?;
                        let from_thread = if let Some(thread) = thread {
                            sqlx::query_scalar::<_,String>("SELECT project_id FROM android_personal_threads WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(thread.to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.map(|s|Uuid::parse_str(&s).map_err(storage_error)).transpose()?
                        } else {
                            None
                        };
                        if explicit.is_some() && from_thread.is_some() && explicit != from_thread {
                            return Err(Error::Conflict);
                        }
                        let Some(project) = explicit.or(from_thread) else {
                            continue;
                        };
                        if !private_parent(store, tx, actor, project).await? {
                            continue;
                        }
                        let runner = payload
                            .get("runner")
                            .and_then(Value::as_str)
                            .unwrap_or("model");
                        if !matches!(runner, "model" | "crew") {
                            return Err(device_invalid());
                        }
                        let mut metadata = serde_json::Map::new();
                        for name in [
                            "task_kind",
                            "expected_output",
                            "thread_id",
                            "runner",
                            "crew_id",
                            "model",
                            "backend_selection",
                            "schedule_expression",
                            "schedule_enabled",
                            "status",
                            "note",
                            "skill_ids",
                            "memory_ids",
                            "mcp_metadata_ids",
                        ] {
                            if let Some(v) = payload.get(name) {
                                metadata.insert(name.into(), v.clone());
                            }
                        }
                        metadata.insert("runner".into(), json!(runner));
                        let config = payload
                            .get("config")
                            .cloned()
                            .unwrap_or_else(|| json!({"sync_metadata":metadata}));
                        let caps = payload
                            .get("required_capabilities")
                            .cloned()
                            .unwrap_or_else(|| json!([]));
                        let array = caps.as_array().ok_or_else(device_invalid)?;
                        if array.len() > 64
                            || array
                                .iter()
                                .any(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 100))
                        {
                            return Err(device_invalid());
                        }
                        let default = payload
                            .get("default_executor_target")
                            .or_else(|| payload.get("default_target"))
                            .cloned()
                            .unwrap_or(Value::Null);
                        target(&default)?;
                        let value = json!({"schema_version":2,"id":id,"revision":revision,"etag":format!("{id}:{revision}"),"project_id":project,"name":text(payload,&["title","name"],200,false)?,"instructions":text(payload,&["description","instructions"],128*1024,false)?,"required_capabilities":caps,"default_target":default,"config":config,"released":true,"created_at":timestamp,"deleted_at":null});
                        let existing: Option<String> = sqlx::query_scalar(
                            "SELECT payload FROM android_task_versions WHERE id=$1 AND revision=$2",
                        )
                        .bind(id.to_string())
                        .bind(revision)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_err(storage_error)?;
                        if let Some(existing) = existing {
                            let old: Value = parse(&existing)?;
                            if old["name"] != value["name"]
                                || old["instructions"] != value["instructions"]
                                || old["config"] != value["config"]
                                || old["required_capabilities"] != value["required_capabilities"]
                                || old["default_target"] != value["default_target"]
                            {
                                return Err(Error::Conflict);
                            }
                        } else {
                            sqlx::query("INSERT INTO android_task_versions(id,revision,project_id,payload,released,deleted) VALUES($1,$2,$3,$4,1,0)").bind(id.to_string()).bind(revision).bind(project.to_string()).bind(json_string(&value)?).execute(&mut **tx).await.map_err(storage_error)?;
                        }
                    }
                }
                "message" => {
                    if deleted {
                        sqlx::query("UPDATE android_personal_messages SET revision=$1,deleted=1 WHERE id=$2 AND owner_id=$3").bind(revision).bind(id.to_string()).bind(actor.identity_id().to_string()).execute(&mut **tx).await.map_err(storage_error)?;
                    } else {
                        let thread = wire_uuid(payload, "thread_id")?;
                        let project:Option<String>=sqlx::query_scalar("SELECT project_id FROM android_personal_threads WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(thread.to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
                        let Some(project) = project else {
                            continue;
                        };
                        let project = Uuid::parse_str(&project).map_err(storage_error)?;
                        if !private_parent(store, tx, actor, project).await? {
                            continue;
                        }
                        let role = payload
                            .get("role")
                            .and_then(Value::as_str)
                            .unwrap_or("assistant");
                        if !matches!(role, "user" | "assistant" | "system" | "tool") {
                            return Err(device_invalid());
                        }
                        let created = payload
                            .get("timestamp")
                            .and_then(Value::as_i64)
                            .map(device_iso)
                            .transpose()?
                            .unwrap_or_else(|| timestamp.clone());
                        let value = json!({"schema_version":2,"id":id,"revision":revision,"etag":digest,"thread_id":thread,"author_user_id":if role=="user"{Some(actor.identity_id())}else{None},"role":role,"content":payload,"run_id":null,"created_at":created,"updated_at":timestamp,"deleted_at":null});
                        sqlx::query("INSERT INTO android_personal_messages(id,owner_id,project_id,thread_id,revision,payload,deleted) VALUES($1,$2,$3,$4,$5,$6,0) ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id,thread_id=excluded.thread_id,revision=excluded.revision,payload=excluded.payload,deleted=0").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(project.to_string()).bind(thread.to_string()).bind(revision).bind(json_string(&value)?).execute(&mut **tx).await.map_err(storage_error)?;
                    }
                }
                _ => unreachable!(),
            }
            mark(tx, actor.identity_id(), kind, id, &digest).await?;
        }
    }
    Ok(())
}
impl Store {
    pub async fn android_personal_messages(
        &self,
        actor: &Principal,
        thread: Uuid,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let project:String=sqlx::query_scalar("SELECT project_id FROM android_personal_threads WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(thread.to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        self.android_personal_permit(
            &mut tx,
            actor,
            Uuid::parse_str(&project).map_err(storage_error)?,
            ContentAction::Read,
        )
        .await?;
        let rows:Vec<String>=sqlx::query_scalar("SELECT payload FROM android_personal_messages WHERE owner_id=$1 AND thread_id=$2 AND deleted=0 ORDER BY id LIMIT 10001").bind(actor.identity_id().to_string()).bind(thread.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 10000 {
            return Err(Error::BudgetExceeded);
        }
        let result = rows
            .into_iter()
            .map(|s| parse(&s))
            .collect::<Result<Vec<Value>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
