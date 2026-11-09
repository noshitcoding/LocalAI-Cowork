//! Shared content materialization keeps the original source owner and the actual requesting Principal.
use super::*;
use crate::android_devices::{device_invalid, device_iso};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction};
fn field(v: &Value, names: &[&str], max: usize, empty: bool) -> Result<String> {
    let s = names
        .iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .unwrap_or("");
    if s.len() > max || s.contains('\0') || !empty && s.trim().is_empty() {
        return Err(device_invalid());
    }
    Ok(s.into())
}
fn optional(v: &Value, k: &str) -> Result<Option<Uuid>> {
    v.get(k)
        .filter(|v| !v.is_null())
        .map(|v| {
            Uuid::parse_str(v.as_str().ok_or_else(device_invalid)?).map_err(|_| device_invalid())
        })
        .transpose()
}
pub(crate) async fn seed_current(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    kind: &str,
    id: Uuid,
    current: i64,
) -> Result<()> {
    let sql = match kind {
        "project" => {
            "SELECT payload,revision,deleted FROM android_personal_projects WHERE id=$1 AND id=$2"
        }
        "thread" => {
            "SELECT payload,revision,deleted FROM android_personal_threads WHERE id=$1 AND project_id=$2"
        }
        "task" => {
            "SELECT payload,revision,deleted FROM android_task_versions WHERE id=$1 AND project_id=$2 ORDER BY revision DESC LIMIT 1"
        }
        "message" => {
            "SELECT payload,revision,deleted FROM android_personal_messages WHERE id=$1 AND project_id=$2"
        }
        _ => return Err(device_invalid()),
    };
    let Some(r) = sqlx::query(sql)
        .bind(id.to_string())
        .bind(project.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
    else {
        return Ok(());
    };
    let revision: i64 = r.try_get("revision").map_err(storage_error)?;
    let known:Option<i64>=sqlx::query_scalar("SELECT revision FROM android_wire_sync_current WHERE owner_id=$1 AND entity_type=$2 AND entity_id=$3 AND team_id IS NULL").bind(owner.to_string()).bind(kind).bind(id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    if known.is_none_or(|v| v < revision) {
        let value: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        crate::android_wire_sync::record(
            tx,
            owner,
            kind,
            id,
            None,
            Some(project),
            revision,
            &value,
            r.try_get::<i64, _>("deleted").map_err(storage_error)? == 1,
            current,
        )
        .await?;
    }
    Ok(())
}
pub(crate) async fn scope(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    kind: &str,
    id: Uuid,
    payload: Option<&Value>,
) -> Result<Option<(Uuid, Uuid)>> {
    if !matches!(kind, "project" | "thread" | "task" | "message") {
        return Ok(None);
    }
    let existing:Option<String>=match kind {
        "project"=>sqlx::query_scalar("SELECT id FROM android_personal_projects WHERE id=$1").bind(id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?,
        "thread"=>sqlx::query_scalar("SELECT project_id FROM android_personal_threads WHERE id=$1").bind(id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?,
        "task"=>sqlx::query_scalar("SELECT project_id FROM android_task_versions WHERE id=$1 ORDER BY revision DESC LIMIT 1").bind(id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?,
        _=>sqlx::query_scalar("SELECT project_id FROM android_personal_messages WHERE id=$1").bind(id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?,
    };
    let mut declared = payload
        .map(|v| optional(v, "project_id"))
        .transpose()?
        .flatten();
    if kind == "project" {
        declared = Some(id)
    }
    if declared.is_none() {
        if let Some(thread) = payload
            .map(|v| optional(v, "thread_id"))
            .transpose()?
            .flatten()
        {
            let p: Option<String> = sqlx::query_scalar(
                "SELECT project_id FROM android_personal_threads WHERE id=$1 AND deleted=0",
            )
            .bind(thread.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
            declared = p
                .map(|v| Uuid::parse_str(&v).map_err(storage_error))
                .transpose()?;
        }
    }
    let original = existing
        .map(|v| Uuid::parse_str(&v).map_err(storage_error))
        .transpose()?;
    if original.zip(declared).is_some_and(|(a, b)| a != b) {
        return Err(Error::Conflict);
    }
    let Some(project) = original.or(declared) else {
        return Ok(None);
    };
    let p: Option<String> = sqlx::query_scalar(
        "SELECT owner_id FROM android_personal_projects WHERE id=$1 AND deleted=0",
    )
    .bind(project.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?;
    let Some(owner) = p else { return Ok(None) };
    let owner = Uuid::parse_str(&owner).map_err(storage_error)?;
    store
        .android_personal_permit(tx, actor, project, ContentAction::Write)
        .await?;
    Ok((owner != actor.identity_id()).then_some((owner, project)))
}
pub(crate) async fn apply(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    owner: Uuid,
    project: Uuid,
    kind: &str,
    id: Uuid,
    payload: &Value,
    revision: i64,
    deleted: bool,
    current: i64,
) -> Result<()> {
    store
        .android_personal_permit(tx, actor, project, ContentAction::Write)
        .await?;
    apply_content(
        tx,
        actor.identity_id(),
        owner,
        project,
        kind,
        id,
        payload,
        revision,
        deleted,
        current,
    )
    .await?;
    // Deleted parent remains inaccessible; no deleted content is returned as an execution permit.
    if !deleted || kind != "project" {
        store
            .android_personal_permit(tx, actor, project, ContentAction::Write)
            .await?;
    } else {
        store
            .identity()
            .require_global_session_in_transaction(tx, actor)
            .await
            .map_err(team_service::identity_error)?;
    }
    Ok(())
}

/// Internal caller must possess its own fresh transaction-bound content authority.
/// No Principal, grant or credential is manufactured by this content-only projection.
pub(crate) async fn apply_content(
    tx: &mut Transaction<'_, Any>,
    executing_actor: Uuid,
    owner: Uuid,
    project: Uuid,
    kind: &str,
    id: Uuid,
    payload: &Value,
    revision: i64,
    deleted: bool,
    current: i64,
) -> Result<()> {
    let timestamp = device_iso(current)?;
    match kind {
        "project" => {
            if !deleted && payload.get("privacy").is_some_and(|v| v != "private_local") {
                return Err(device_invalid());
            }
            let old:String=sqlx::query_scalar("SELECT payload FROM android_personal_projects WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(project.to_string()).bind(owner.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
            let mut v: Value = parse(&old)?;
            v["revision"] = json!(revision);
            v["etag"] = json!(format!("{id}:{revision}"));
            v["updated_at"] = json!(timestamp);
            if deleted {
                crate::android_wire_materialization::delete_project_children(
                    tx, owner, project, current,
                )
                .await?;
                v["deleted_at"] = json!(timestamp);
            } else {
                v["name"] = json!(field(payload, &["title", "name"], 200, false)?);
                v["description"] = json!(field(
                    payload,
                    &["instructions", "description"],
                    64 * 1024,
                    true
                )?);
                if let Some(policy) = payload.get("policy") {
                    if !policy.is_object() || json_string(policy)?.len() > 64 * 1024 {
                        return Err(device_invalid());
                    }
                    v["policy"] = policy.clone();
                }
                if let Some(target) = payload.get("preferred_executor_target") {
                    v["preferred_executor_target"] = target.clone();
                }
            }
            sqlx::query("UPDATE android_personal_projects SET revision=$1,payload=$2,deleted=$3 WHERE id=$4 AND owner_id=$5").bind(revision).bind(json_string(&v)?).bind(i64::from(deleted)).bind(project.to_string()).bind(owner.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
        }
        "thread" => {
            let old: Option<String> = sqlx::query_scalar(
                "SELECT payload FROM android_personal_threads WHERE id=$1 AND project_id=$2",
            )
            .bind(id.to_string())
            .bind(project.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
            if deleted {
                sqlx::query("UPDATE android_personal_threads SET deleted=1,revision=$1 WHERE id=$2 AND project_id=$3").bind(revision).bind(id.to_string()).bind(project.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
                sqlx::query("UPDATE android_personal_messages SET deleted=1 WHERE thread_id=$1 AND project_id=$2").bind(id.to_string()).bind(project.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
            } else {
                let created = old
                    .as_deref()
                    .map(parse::<Value>)
                    .transpose()?
                    .and_then(|v| v.get("created_at").cloned())
                    .unwrap_or_else(|| json!(timestamp));
                let v = json!({"schema_version":2,"id":id,"revision":revision,"etag":format!("{id}:{revision}"),"project_id":project,"forked_from_thread_id":null,"forked_from_message_id":null,"title":field(payload,&["title"],200,false)?,"created_at":created,"deleted_at":null});
                sqlx::query("INSERT INTO android_personal_threads(id,project_id,owner_id,revision,payload,deleted) VALUES($1,$2,$3,$4,$5,0) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload,deleted=0 WHERE android_personal_threads.project_id=$2").bind(id.to_string()).bind(project.to_string()).bind(owner.to_string()).bind(revision).bind(json_string(&v)?).execute(&mut **tx).await.map_err(storage_error)?;
            }
        }
        "task" => {
            if deleted {
                sqlx::query("UPDATE android_task_versions SET deleted=1,released=0 WHERE id=$1 AND project_id=$2").bind(id.to_string()).bind(project.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
            } else {
                let config = payload
                    .get("config")
                    .cloned()
                    .unwrap_or_else(|| json!({"sync_metadata":payload}));
                if !config.is_object() || json_string(&config)?.len() > 64 * 1024 {
                    return Err(device_invalid());
                }
                let runner = config
                    .get("runner")
                    .or_else(|| config.get("sync_metadata").and_then(|m| m.get("runner")))
                    .and_then(Value::as_str)
                    .unwrap_or("model");
                if !matches!(runner, "model" | "crew" | "workflow") {
                    return Err(device_invalid());
                }
                let caps = payload
                    .get("required_capabilities")
                    .cloned()
                    .unwrap_or_else(|| json!([]));
                if caps.as_array().is_none_or(|a| {
                    a.len() > 64
                        || a.iter()
                            .any(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 100))
                }) {
                    return Err(device_invalid());
                }
                let v = json!({"schema_version":2,"id":id,"revision":revision,"etag":format!("{id}:{revision}"),"project_id":project,"name":field(payload,&["title","name"],200,false)?,"instructions":field(payload,&["description","instructions"],128*1024,false)?,"required_capabilities":caps,"default_target":payload.get("default_target").or_else(||payload.get("default_executor_target")).cloned().unwrap_or(Value::Null),"config":config,"released":true,"created_at":timestamp,"deleted_at":null});
                if let Some(old) = sqlx::query_scalar::<_, String>(
                    "SELECT payload FROM android_task_versions WHERE id=$1 AND revision=$2",
                )
                .bind(id.to_string())
                .bind(revision)
                .fetch_optional(&mut **tx)
                .await
                .map_err(storage_error)?
                {
                    if parse::<Value>(&old)? != v {
                        return Err(Error::Conflict);
                    }
                } else {
                    sqlx::query("INSERT INTO android_task_versions(id,revision,project_id,payload,released,deleted) VALUES($1,$2,$3,$4,1,0)").bind(id.to_string()).bind(revision).bind(project.to_string()).bind(json_string(&v)?).execute(&mut **tx).await.map_err(storage_error)?;
                }
            }
        }
        "message" => {
            if deleted {
                sqlx::query("UPDATE android_personal_messages SET deleted=1,revision=$1 WHERE id=$2 AND project_id=$3").bind(revision).bind(id.to_string()).bind(project.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
            } else {
                let thread = optional(payload, "thread_id")?.ok_or_else(device_invalid)?;
                let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_threads WHERE id=$1 AND project_id=$2 AND deleted=0").bind(thread.to_string()).bind(project.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
                if valid != 1 {
                    return Err(Error::NotFound);
                }
                let role = payload["role"].as_str().unwrap_or("assistant");
                if !matches!(role, "user" | "assistant" | "system" | "tool") {
                    return Err(device_invalid());
                }
                let v = json!({"schema_version":2,"id":id,"revision":revision,"thread_id":thread,"project_id":project,"role":role,"content":payload["content"],"run_id":null,"created_at":payload.get("created_at").cloned().unwrap_or_else(||json!(timestamp)),"deleted_at":null,"metadata":{"source":"synced_shared_content","executing_actor":executing_actor}});
                sqlx::query("INSERT INTO android_personal_messages(id,owner_id,project_id,thread_id,revision,payload,deleted) VALUES($1,$2,$3,$4,$5,$6,0) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload,deleted=0 WHERE android_personal_messages.project_id=$3 AND android_personal_messages.thread_id=$4").bind(id.to_string()).bind(owner.to_string()).bind(project.to_string()).bind(thread.to_string()).bind(revision).bind(json_string(&v)?).execute(&mut **tx).await.map_err(storage_error)?;
            }
        }
        _ => return Err(device_invalid()),
    }
    Ok(())
}
