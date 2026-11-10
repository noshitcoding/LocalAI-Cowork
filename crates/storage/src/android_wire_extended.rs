//! Sync projection of private schedule drafts and provider metadata. Runs remain inert replicas.
use super::*;
use crate::android_devices::{device_invalid, device_iso};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction};

fn uuid(v: &Value, key: &str) -> Result<Uuid> {
    crate::android_personal::wire_uuid(v, key)
}
fn text(v: &Value, key: &str, max: usize) -> Result<String> {
    let s = v[key]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control))
        .ok_or_else(device_invalid)?;
    Ok(s.into())
}
pub(crate) async fn admit(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    kind: &str,
    id: Uuid,
) -> Result<()> {
    if !matches!(kind, "schedule" | "provider_profile" | "run") {
        return Ok(());
    }
    let existing: Option<String> = match kind {
        "schedule" => {
            sqlx::query_scalar("SELECT owner_id FROM android_private_schedules WHERE id=$1")
                .bind(id.to_string())
                .fetch_optional(&mut **tx)
                .await
                .map_err(storage_error)?
        }
        "provider_profile" => sqlx::query_scalar(
            "SELECT owner_id FROM android_personal_provider_profiles WHERE id=$1",
        )
        .bind(id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?,
        _ => None,
    };
    if existing.as_ref().is_some_and(|v| v != &owner.to_string()) {
        return Err(Error::Forbidden);
    }
    let other:Option<String>=sqlx::query_scalar("SELECT owner_id FROM android_wire_projection_sources WHERE entity_type=$1 AND entity_id=$2").bind(kind).bind(id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    if other.as_ref().is_some_and(|v| v != &owner.to_string()) {
        return Err(Error::Conflict);
    }
    if kind == "provider_profile" {
        let secrets: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_personal_provider_secrets WHERE profile_id=$1",
        )
        .bind(id.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
        // Changing a configured profile needs the existing credential-aware product transaction.
        if secrets != 0 {
            return Err(Error::Conflict);
        }
    }
    sqlx::query("INSERT INTO android_wire_projection_sources(owner_id,entity_type,entity_id) VALUES($1,$2,$3) ON CONFLICT(owner_id,entity_type,entity_id) DO NOTHING").bind(owner.to_string()).bind(kind).bind(id.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
pub(crate) async fn materialize(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    current: i64,
) -> Result<()> {
    // One row at a time. A deferred draft is revisited when its project/thread/task arrives later.
    for kind in ["provider_profile", "schedule"] {
        let mut after = String::new();
        loop {
            let Some(row)=sqlx::query("SELECT c.entity_id,c.payload,c.digest,c.revision,c.tombstone,p.applied_digest FROM android_wire_sync_current c JOIN android_wire_projection_sources p ON p.owner_id=c.owner_id AND p.entity_type=c.entity_type AND p.entity_id=c.entity_id WHERE c.owner_id=$1 AND c.entity_type=$2 AND c.team_id IS NULL AND c.entity_id>$3 ORDER BY c.entity_id LIMIT 1").bind(actor.identity_id().to_string()).bind(kind).bind(&after).fetch_optional(&mut **tx).await.map_err(storage_error)? else{break};
            after = row.try_get("entity_id").map_err(storage_error)?;
            let id = Uuid::parse_str(&after).map_err(storage_error)?;
            let digest: String = row.try_get("digest").map_err(storage_error)?;
            if row
                .try_get::<Option<String>, _>("applied_digest")
                .map_err(storage_error)?
                .as_deref()
                == Some(&digest)
            {
                continue;
            }
            let revision: i64 = row.try_get("revision").map_err(storage_error)?;
            let deleted = row.try_get::<i64, _>("tombstone").map_err(storage_error)? == 1;
            let entity: Value =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let payload = &entity["payload"];
            if kind == "provider_profile" {
                let old:Option<String>=sqlx::query_scalar("SELECT payload FROM android_personal_provider_profiles WHERE id=$1 AND owner_id=$2").bind(id.to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
                let mut value = old
                    .as_deref()
                    .map(parse::<Value>)
                    .transpose()?
                    .unwrap_or_else(|| json!({"created_at":device_iso(current).ok()}));
                if deleted {
                    if old.is_some() {
                        value["revision"] = json!(revision);
                        value["has_secret"] = json!(false);
                        value["deleted_at"] = json!(device_iso(current)?);
                        value["updated_at"] = json!(device_iso(current)?);
                    }
                } else {
                    let defaults = payload["model_defaults"]
                        .as_object()
                        .filter(|v| json_string(v).is_ok_and(|s| s.len() <= 64 * 1024))
                        .ok_or_else(device_invalid)?;
                    let binding = defaults
                        .get("endpoint_binding")
                        .and_then(Value::as_str)
                        .unwrap_or("per_device");
                    if !matches!(binding, "per_device" | "server") {
                        return Err(device_invalid());
                    }
                    // No client has_secret flag, registry reference, or source qualification is adopted.
                    let defaults = if binding == "server" {
                        crate::android_personal_provider::normalized_defaults(Value::Object(
                            defaults.clone(),
                        ))?
                    } else {
                        let mut d = defaults.clone();
                        d.remove("model_binding");
                        d.remove("model_registry");
                        d.insert("endpoint_binding".into(), json!("per_device"));
                        d.insert("source".into(), json!("device"));
                        Value::Object(d)
                    };
                    value = json!({"schema_version":2,"id":id,"revision":revision,"etag":format!("{id}:{revision}"),"owner_user_id":actor.identity_id(),"team_id":null,"name":text(payload,"name",200)?,"provider_kind":text(payload,"provider_kind",64)?,"model_defaults":defaults,"has_secret":false,"created_at":value["created_at"],"updated_at":device_iso(current)?,"deleted_at":null});
                }
                if !deleted || old.is_some() {
                    sqlx::query("INSERT INTO android_personal_provider_profiles(id,owner_id,revision,payload,deleted) VALUES($1,$2,$3,$4,$5) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload,deleted=excluded.deleted WHERE android_personal_provider_profiles.owner_id=$2").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(revision).bind(json_string(&value)?).bind(i64::from(deleted)).execute(&mut **tx).await.map_err(storage_error)?;
                    crate::android_personal_provider::versions::persist(
                        tx,
                        actor.identity_id(),
                        id,
                    )
                    .await?;
                    if deleted {
                        crate::android_personal_provider::versions::revoke(
                            tx,
                            actor.identity_id(),
                            id,
                        )
                        .await?;
                    }
                }
            } else {
                let old=sqlx::query("SELECT project_id,thread_id,execution_revision,grant_id,payload FROM android_private_schedules WHERE id=$1 AND owner_id=$2").bind(id.to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
                let mut value = old
                    .as_ref()
                    .map(|r| {
                        parse::<Value>(&r.try_get::<String, _>("payload").map_err(storage_error)?)
                    })
                    .transpose()?
                    .unwrap_or(Value::Null);
                let (project, thread) = if deleted {
                    match &old {
                        Some(r) => (
                            Uuid::parse_str(
                                &r.try_get::<String, _>("project_id")
                                    .map_err(storage_error)?,
                            )
                            .map_err(storage_error)?,
                            Uuid::parse_str(
                                &r.try_get::<String, _>("thread_id").map_err(storage_error)?,
                            )
                            .map_err(storage_error)?,
                        ),
                        None => {
                            sqlx::query("UPDATE android_wire_projection_sources SET applied_digest=$1 WHERE owner_id=$2 AND entity_type=$3 AND entity_id=$4").bind(&digest).bind(actor.identity_id().to_string()).bind(kind).bind(&after).execute(&mut **tx).await.map_err(storage_error)?;
                            continue;
                        }
                    }
                } else {
                    (uuid(payload, "project_id")?, uuid(payload, "thread_id")?)
                };
                let exists: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM android_personal_projects WHERE id=$1 AND deleted=0",
                )
                .bind(project.to_string())
                .fetch_one(&mut **tx)
                .await
                .map_err(storage_error)?;
                if exists != 1 {
                    continue;
                }
                store
                    .android_personal_permit(tx, actor, project, ContentAction::Write)
                    .await?;
                if !deleted {
                    let parent:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_threads t JOIN android_task_versions v ON v.project_id=t.project_id WHERE t.id=$1 AND t.project_id=$2 AND t.deleted=0 AND v.id=$3 AND v.deleted=0").bind(thread.to_string()).bind(project.to_string()).bind(uuid(payload,"task_id")?.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
                    if parent == 0 {
                        continue;
                    }
                    let cron = text(payload, "cron", 512)?;
                    let timezone = text(payload, "timezone", 128)?;
                    let cron = crate::android_schedule_cron::normalize(&cron, &timezone)?;
                    crate::android_schedule_cron::next(&cron, &timezone, current)?;
                    let target = payload["executor_target"]
                        .as_object()
                        .ok_or_else(device_invalid)?;
                    if !matches!(
                        target.get("kind").and_then(Value::as_str),
                        Some("server_linux" | "personal_device" | "managed_windows_pool")
                    ) {
                        return Err(device_invalid());
                    }
                    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
                    if json_string(&input)?.len() > 128 * 1024 {
                        return Err(device_invalid());
                    }
                    value = json!({"schema_version":2,"id":id,"revision":revision,"etag":format!("{id}:{revision}"),"task_id":uuid(payload,"task_id")?,"project_id":project,"thread_id":thread,"cron":cron,"timezone":timezone,"executor_target":target,"input":input,"model_profile_id":payload["model_profile_id"],"enabled":false,"next_run_at":null,"last_triggered_at":value["last_triggered_at"],"blocked_reason":"synced_schedule_requires_fresh_grant","created_at":value.get("created_at").cloned().unwrap_or_else(||json!(device_iso(current).ok())),"updated_at":device_iso(current)?,"deleted_at":null});
                } else {
                    value["revision"] = json!(revision);
                    value["enabled"] = json!(false);
                    value["next_run_at"] = Value::Null;
                    value["deleted_at"] = json!(device_iso(current)?);
                    value["updated_at"] = json!(device_iso(current)?);
                }
                // Revoke a previous schedule grant; existing original occurrences and uncertain effects survive.
                if let Some(r) = &old
                    && let Some(grant) = r
                        .try_get::<Option<String>, _>("grant_id")
                        .map_err(storage_error)?
                {
                    sqlx::query("UPDATE android_private_schedule_grants SET active=0 WHERE id=$1 AND owner_id=$2").bind(grant).bind(actor.identity_id().to_string()).execute(&mut **tx).await.map_err(storage_error)?;
                }
                let execution = old
                    .as_ref()
                    .map(|r| {
                        r.try_get::<i64, _>("execution_revision")
                            .map_err(storage_error)
                    })
                    .transpose()?
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or(Error::Conflict)?;
                sqlx::query("INSERT INTO android_private_schedules(id,owner_id,project_id,thread_id,revision,execution_revision,grant_id,enabled,deleted,next_run,active_occurrence_id,payload) VALUES($1,$2,$3,$4,$5,$6,NULL,0,$7,NULL,NULL,$8) ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id,thread_id=excluded.thread_id,revision=excluded.revision,execution_revision=excluded.execution_revision,grant_id=NULL,enabled=0,deleted=excluded.deleted,next_run=NULL,payload=excluded.payload WHERE android_private_schedules.owner_id=$2").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(project.to_string()).bind(thread.to_string()).bind(revision).bind(execution).bind(i64::from(deleted)).bind(json_string(&value)?).execute(&mut **tx).await.map_err(storage_error)?;
            }
            sqlx::query("UPDATE android_wire_projection_sources SET applied_digest=$1 WHERE owner_id=$2 AND entity_type=$3 AND entity_id=$4").bind(digest).bind(actor.identity_id().to_string()).bind(kind).bind(after.clone()).execute(&mut **tx).await.map_err(storage_error)?;
        }
    }
    store
        .identity()
        .require_session_action_in_transaction(tx, actor, ContentAction::Write)
        .await
        .map_err(team_service::identity_error)
}
