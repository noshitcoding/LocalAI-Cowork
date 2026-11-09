//! One-shot device leases. Unknown or expired execution is never automatically reissued.
use super::*;
use crate::android_device_registry::{AndroidAgentPrincipal, agent_current};
use crate::android_devices::{device_invalid, device_iso, device_writer};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction, any::AnyRow};
use team_service::identity_error;

pub struct AndroidDeviceRunRequest {
    pub team: Option<Uuid>,
    pub project: Uuid,
    pub order: Uuid,
    pub project_revision: i64,
    pub target: Value,
    pub capabilities: Vec<String>,
    pub input: Value,
    pub model_profile: Option<Uuid>,
    pub snapshot: Option<Uuid>,
    pub key: String,
    pub task: Option<Value>,
}
fn uuid(r: &AnyRow, key: &str) -> Result<Uuid> {
    Uuid::parse_str(&r.try_get::<String, _>(key).map_err(storage_error)?).map_err(storage_error)
}
pub(crate) async fn device_run_row(tx: &mut Transaction<'_, Any>, id: Uuid) -> Result<AnyRow> {
    sqlx::query("SELECT * FROM android_scoped_runs WHERE id=$1")
        .bind(id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)
}
pub(crate) async fn device_run_permit(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    r: &AnyRow,
    action: ContentAction,
) -> Result<()> {
    if r.try_get::<Option<String>, _>("team_id")
        .map_err(storage_error)?
        .is_none()
    {
        if uuid(r, "creator_id")? != actor.identity_id() {
            return Err(Error::Forbidden);
        }
        store
            .android_personal_permit(tx, actor, uuid(r, "project_id")?, action)
            .await?;
        return Ok(());
    }
    let p = store
        .identity()
        .authorize_content_in_transaction(
            tx,
            actor,
            uuid(r, "team_id")?,
            uuid(r, "project_id")?,
            action,
        )
        .await
        .map_err(identity_error)?;
    store
        .identity()
        .validate_content_in_transaction(tx, &p)
        .await
        .map_err(identity_error)
}
pub(crate) async fn device_run_authority(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    current: i64,
) -> Result<()> {
    let payload: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if let Some(task) = payload.pointer("/spec/task").filter(|v| !v.is_null()) {
        let id = crate::android_personal::wire_uuid(task, "id")?;
        let revision = task["revision"]
            .as_i64()
            .filter(|r| *r > 0)
            .ok_or_else(device_invalid)?;
        let task:String=sqlx::query_scalar("SELECT payload FROM android_task_versions WHERE id=$1 AND revision=$2 AND project_id=$3 AND released=1 AND deleted=0").bind(id.to_string()).bind(revision).bind(r.try_get::<String,_>("project_id").map_err(storage_error)?).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let task: Value = parse(&task)?;
        if payload.pointer("/spec/input/task_instructions") != task.get("instructions")
            || payload.pointer("/spec/input/task_config") != task.get("config")
        {
            return Err(Error::Forbidden);
        }
    }
    if r.try_get::<Option<String>, _>("team_id")
        .map_err(storage_error)?
        .is_none()
    {
        if crate::android_private_schedules::validate_origin(tx, r, "personal_device", current)
            .await?
        {
            crate::android_private_snapshots::validate_files(
                tx,
                uuid(r, "creator_id")?,
                uuid(r, "project_id")?,
                &payload["spec"]["input"],
                current,
            )
            .await?;
            return Ok(());
        }
        let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM identity_subjects i JOIN identity_sessions s ON s.identity_id=i.id JOIN android_personal_projects p ON p.owner_id=i.id JOIN android_personal_threads t ON t.project_id=p.id AND t.owner_id=i.id WHERE i.id=$1 AND i.active=1 AND s.id=$2 AND s.revision=$3 AND s.revoked=0 AND s.expires_at>$4 AND NOT EXISTS(SELECT 1 FROM identity_session_scopes ss WHERE ss.session_id=s.id) AND p.id=$5 AND p.revision=$6 AND p.deleted=0 AND t.id=$7 AND t.deleted=0")
            .bind(r.try_get::<String,_>("creator_id").map_err(storage_error)?).bind(r.try_get::<String,_>("creator_session_id").map_err(storage_error)?).bind(r.try_get::<i64,_>("creator_session_revision").map_err(storage_error)?).bind(current).bind(r.try_get::<String,_>("project_id").map_err(storage_error)?).bind(r.try_get::<i64,_>("project_revision").map_err(storage_error)?).bind(r.try_get::<String,_>("order_id").map_err(storage_error)?).fetch_one(&mut **tx).await.map_err(storage_error)?;
        if valid != 1 {
            return Err(Error::Forbidden);
        }
        crate::android_private_snapshots::validate_files(
            tx,
            uuid(r, "creator_id")?,
            uuid(r, "project_id")?,
            &payload["spec"]["input"],
            current,
        )
        .await?;
        if let Some(snapshot) = payload["spec"]["snapshot_id"].as_str() {
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_snapshots WHERE id=$1 AND owner_id=$2 AND project_id=$3").bind(snapshot).bind(r.try_get::<String,_>("creator_id").map_err(storage_error)?).bind(r.try_get::<String,_>("project_id").map_err(storage_error)?).fetch_one(&mut **tx).await.map_err(storage_error)?;
            if count == 1 {
                crate::android_private_snapshots::manifest(
                    tx,
                    uuid(r, "creator_id")?,
                    uuid(r, "project_id")?,
                    Uuid::parse_str(snapshot).map_err(storage_error)?,
                    true,
                    current,
                )
                .await?;
            }
        }
        return Ok(());
    }
    let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM identity_subjects i JOIN identity_sessions s ON s.identity_id=i.id JOIN identity_memberships m ON m.identity_id=i.id JOIN identity_organizations o ON o.id=m.organization_id JOIN identity_resources p ON p.organization_id=o.id JOIN identity_content_grants g ON g.membership_id=m.id AND g.resource_id=p.id JOIN tenant_projects tp ON tp.id=p.id LEFT JOIN team_project_profiles f ON f.project_id=p.id JOIN team_orders t ON t.project_id=p.id WHERE i.id=$1 AND s.id=$2 AND s.revision=$3 AND s.revoked=0 AND s.expires_at>$4 AND i.active=1 AND m.active=1 AND m.role<>'viewer' AND o.id=$5 AND o.state='active' AND o.authorization_epoch=$6 AND p.id=$7 AND g.active=1 AND (g.permissions & 4)=4 AND COALESCE(f.revision,1)=$8 AND COALESCE(f.archived,0)=0 AND t.id=$9")
        .bind(r.try_get::<String,_>("creator_id").map_err(storage_error)?).bind(r.try_get::<String,_>("creator_session_id").map_err(storage_error)?).bind(r.try_get::<i64,_>("creator_session_revision").map_err(storage_error)?).bind(current).bind(r.try_get::<String,_>("team_id").map_err(storage_error)?).bind(r.try_get::<i64,_>("authorization_epoch").map_err(storage_error)?).bind(r.try_get::<String,_>("project_id").map_err(storage_error)?).bind(r.try_get::<i64,_>("project_revision").map_err(storage_error)?).bind(r.try_get::<String,_>("order_id").map_err(storage_error)?).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if valid != 1 {
        return Err(Error::Forbidden);
    }
    let order: String = sqlx::query_scalar(
        "SELECT payload FROM team_orders WHERE id=$1 AND team_id=$2 AND project_id=$3",
    )
    .bind(r.try_get::<String, _>("order_id").map_err(storage_error)?)
    .bind(r.try_get::<String, _>("team_id").map_err(storage_error)?)
    .bind(
        r.try_get::<String, _>("project_id")
            .map_err(storage_error)?,
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if parse::<TeamOrder>(&order)?.archived {
        return Err(Error::Conflict);
    }
    Ok(())
}
pub(crate) async fn device_lease_current(
    tx: &mut Transaction<'_, Any>,
    a: &AndroidAgentPrincipal,
    id: Uuid,
    token: Uuid,
    current: i64,
) -> Result<AnyRow> {
    agent_current(tx, a, current).await?;
    let r = device_run_row(tx, id).await?;
    if uuid(&r, "executor_id")? != a.executor_id
        || r.try_get::<i64, _>("executor_revision")
            .map_err(storage_error)?
            != a.device_revision
        || r.try_get::<Option<String>, _>("credential_id")
            .map_err(storage_error)?
            != Some(a.credential_id.to_string())
        || r.try_get::<Option<i64>, _>("credential_revision")
            .map_err(storage_error)?
            != Some(a.credential_revision)
        || r.try_get::<Option<String>, _>("lease_token")
            .map_err(storage_error)?
            != Some(token.to_string())
        || !matches!(
            r.try_get::<String, _>("state")
                .map_err(storage_error)?
                .as_str(),
            "leased" | "running"
        )
        || r.try_get::<Option<i64>, _>("lease_expires_at")
            .map_err(storage_error)?
            .is_none_or(|e| e <= current)
    {
        return Err(Error::Forbidden);
    }
    device_run_authority(tx, &r, current).await?;
    Ok(r)
}
pub(crate) async fn device_event(
    tx: &mut Transaction<'_, Any>,
    id: Uuid,
    source: Option<Uuid>,
    kind: &str,
    payload: &Value,
    current: i64,
) -> Result<Value> {
    if kind.is_empty()
        || kind.len() > 100
        || !kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        || json_string(payload)?.len() > 512 * 1024
    {
        return Err(device_invalid());
    }
    let digest = hash(json_string(&(kind, payload))?.as_bytes());
    if let Some(source) = source {
        if let Some(row)=sqlx::query("SELECT request_sha256,payload FROM android_scoped_events WHERE run_id=$1 AND source_event_id=$2").bind(id.to_string()).bind(source.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)? {if row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=digest{return Err(Error::Conflict)}return parse(&row.try_get::<String,_>("payload").map_err(storage_error)?)}
    }
    let sequence: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(sequence),0)+1 FROM android_scoped_events WHERE run_id=$1",
    )
    .bind(id.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if sequence > 100_000 {
        return Err(Error::BudgetExceeded);
    }
    let event = Uuid::new_v4();
    let v = json!({"schema_version":2,"run_id":id,"sequence":sequence,"event_id":event,"kind":kind,"payload":payload,"created_at":device_iso(current)?});
    sqlx::query("INSERT INTO android_scoped_events(run_id,sequence,event_id,source_event_id,request_sha256,payload) VALUES($1,$2,$3,$4,$5,$6)").bind(id.to_string()).bind(sequence).bind(event.to_string()).bind(source.map(|s|s.to_string())).bind(digest).bind(json_string(&v)?).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(v)
}
fn updated(r: &AnyRow, state: &str, current: i64) -> Result<Value> {
    let mut v: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    v["state"] = json!(state);
    v["revision"] = json!(r.try_get::<i64, _>("revision").map_err(storage_error)? + 1);
    v["etag"] = json!(format!(
        "{}:{}",
        r.try_get::<String, _>("id").map_err(storage_error)?,
        r.try_get::<i64, _>("revision").map_err(storage_error)? + 1
    ));
    v["updated_at"] = json!(device_iso(current)?);
    Ok(v)
}
pub(crate) async fn save_run(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    v: &Value,
    state: &str,
    current: i64,
) -> Result<()> {
    let n=sqlx::query("UPDATE android_scoped_runs SET payload=$1,state=$2,revision=revision+1,updated_at=$3 WHERE id=$4 AND revision=$5").bind(json_string(v)?).bind(state).bind(current).bind(r.try_get::<String,_>("id").map_err(storage_error)?).bind(r.try_get::<i64,_>("revision").map_err(storage_error)?).execute(&mut **tx).await.map_err(storage_error)?;
    if n.rows_affected() != 1 {
        return Err(Error::Conflict);
    }
    Ok(())
}
impl Store {
    pub(crate) async fn android_device_create_scheduled(
        &self,
        tx: &mut Transaction<'_, Any>,
        q: &crate::android_private_schedules::ScheduledRun,
        current: i64,
    ) -> Result<Value> {
        crate::android_private_schedules::validate_prepared(tx, q, current).await?;
        if q.target["kind"] != "personal_device" || q.profile.is_some() {
            return Err(Error::Forbidden);
        }
        let executor = crate::android_personal::wire_uuid(&q.target, "device_id")?;
        let revision:i64=sqlx::query_scalar("SELECT execution_revision FROM android_devices WHERE id=$1 AND identity_id=$2 AND team_id IS NULL AND revoked=0 AND draining=0").bind(executor.to_string()).bind(q.owner.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let sha = hash(json_string(q)?.as_bytes());
        if let Some(row)=sqlx::query("SELECT creator_id,creation_key,request_sha256,payload FROM android_scoped_runs WHERE id=$1").bind(q.run.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?{if row.try_get::<String,_>("creator_id").map_err(storage_error)?!=q.owner.to_string()||row.try_get::<String,_>("creation_key").map_err(storage_error)?!=q.key||row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=sha{return Err(Error::Conflict);}return parse(&row.try_get::<String,_>("payload").map_err(storage_error)?);}
        let spec = json!({"schema_version":2,"id":q.run,"thread_id":q.thread,"project_id":q.project,"project":{"id":q.project,"revision":q.project_revision},"project_revision":q.project_revision,"project_privacy":"private_local","task":q.task,"creator_user_id":q.owner,"executor_target":q.target,"required_capabilities":q.capabilities,"input":q.input,"model_profile_id":null,"snapshot_id":q.snapshot,"idempotency_key":q.key,"created_at":device_iso(current)?});
        let value = json!({"spec":spec,"state":"queued","revision":1,"etag":format!("{}:1",q.run),"assigned_executor_id":executor,"lease_expires_at":null,"started_at":null,"finished_at":null,"result":null,"error":null,"updated_at":device_iso(current)?});
        sqlx::query("INSERT INTO android_scoped_runs(id,team_id,project_id,order_id,creator_id,creator_session_id,creator_session_revision,authorization_epoch,project_revision,executor_id,executor_revision,creation_key,request_sha256,payload,state,revision,created_at,updated_at) VALUES($1,NULL,$2,$3,$4,$5,$6,0,$7,$8,$9,$10,$11,$12,'queued',1,$13,$13)").bind(q.run.to_string()).bind(q.project.to_string()).bind(q.thread.to_string()).bind(q.owner.to_string()).bind(q.session.to_string()).bind(q.session_revision).bind(q.project_revision).bind(executor.to_string()).bind(revision).bind(&q.key).bind(sha).bind(json_string(&value)?).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
        device_event(
            tx,
            q.run,
            None,
            "run_created",
            &json!({"state":"queued","schedule_origin":q.input["schedule_origin"]}),
            current,
        )
        .await?;
        Ok(value)
    }
    pub async fn android_device_create_run(
        &self,
        actor: &Principal,
        q: AndroidDeviceRunRequest,
    ) -> Result<Value> {
        self.android_device_create_run_with_files(actor, q, Vec::new())
            .await
    }
    /// Freeze selected, ACL-checked project versions before making the run claimable.
    pub async fn android_device_create_run_with_files(
        &self,
        actor: &Principal,
        q: AndroidDeviceRunRequest,
        files: Vec<(String, Vec<u8>)>,
    ) -> Result<Value> {
        if q.key
            .starts_with(crate::android_private_schedules::KEY_PREFIX)
        {
            return Err(Error::Forbidden);
        }
        if files.len() > 2000 || (q.snapshot.is_some() && !files.is_empty()) {
            return Err(device_invalid());
        }
        let mut paths = std::collections::BTreeSet::new();
        let mut total = 0usize;
        let mut inventory = Vec::new();
        for (path, bytes) in &files {
            crate::android_device_content::relative(path)?;
            if !paths.insert(path) || bytes.len() > 32 * 1024 * 1024 {
                return Err(device_invalid());
            }
            total = total
                .checked_add(bytes.len())
                .ok_or(Error::BudgetExceeded)?;
            if total > 256 * 1024 * 1024 {
                return Err(Error::BudgetExceeded);
            }
            inventory.push(json!({"path":path,"size":bytes.len(),"digest":hash(bytes),"chunks":bytes.chunks(4*1024*1024).map(|chunk|json!({"digest":hash(chunk),"plaintext_size":chunk.len(),"ciphertext_size":chunk.len()})).collect::<Vec<_>>() }));
        }
        if q.key.is_empty()
            || q.key.len() > 200
            || q.key.chars().any(char::is_control)
            || q.capabilities.len() > 64
            || json_string(&q.input)?.len() > 256 * 1024
        {
            return Err(device_invalid());
        }
        if let Some(value) = q.input.get("output").filter(|value| !value.is_null()) {
            let output: StructuredOutput =
                serde_json::from_value(value.clone()).map_err(|_| device_invalid())?;
            cowork_domain::structured_output::validate_schema(&output)?;
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let permit = if let Some(team) = q.team {
            let permit = self
                .identity()
                .authorize_content_in_transaction(
                    &mut tx,
                    actor,
                    team,
                    q.project,
                    ContentAction::Execute,
                )
                .await
                .map_err(identity_error)?;
            let profile=sqlx::query("SELECT COALESCE(f.revision,1) AS revision,COALESCE(f.archived,0) AS archived FROM tenant_projects p LEFT JOIN team_project_profiles f ON f.project_id=p.id WHERE p.id=$1 AND p.organization_id=$2").bind(q.project.to_string()).bind(team.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            if profile
                .try_get::<i64, _>("revision")
                .map_err(storage_error)?
                != q.project_revision
                || profile
                    .try_get::<i64, _>("archived")
                    .map_err(storage_error)?
                    != 0
            {
                return Err(Error::Conflict);
            }
            let order: String = sqlx::query_scalar(
                "SELECT payload FROM team_orders WHERE id=$1 AND team_id=$2 AND project_id=$3",
            )
            .bind(q.order.to_string())
            .bind(team.to_string())
            .bind(q.project.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
            if parse::<TeamOrder>(&order)?.archived {
                return Err(Error::Conflict);
            }
            Some(permit)
        } else {
            let project = self
                .android_personal_permit(&mut tx, actor, q.project, ContentAction::Execute)
                .await?;
            if project["revision"].as_i64() != Some(q.project_revision) {
                return Err(Error::Conflict);
            }
            let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_threads WHERE id=$1 AND project_id=$2 AND owner_id=$3 AND deleted=0").bind(q.order.to_string()).bind(q.project.to_string()).bind(actor.identity_id().to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if valid != 1 {
                return Err(Error::NotFound);
            }
            if q.target["kind"] != "personal_device" || q.model_profile.is_some() {
                return Err(device_invalid());
            }
            None
        };
        if let Some(reference) = q.task.as_ref() {
            let id = crate::android_personal::wire_uuid(reference, "id")?;
            let revision = reference["revision"]
                .as_i64()
                .filter(|r| *r > 0)
                .ok_or_else(device_invalid)?;
            let task:String=sqlx::query_scalar("SELECT payload FROM android_task_versions WHERE id=$1 AND revision=$2 AND project_id=$3 AND released=1 AND deleted=0").bind(id.to_string()).bind(revision).bind(q.project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
            let task: Value = parse(&task)?;
            if q.input.get("task_instructions") != task.get("instructions")
                || q.input.get("task_config") != task.get("config")
                || task["required_capabilities"].as_array().is_none_or(|caps| {
                    caps.iter().any(|cap| {
                        cap.as_str()
                            .is_none_or(|cap| !q.capabilities.iter().any(|c| c == cap))
                    })
                })
            {
                return Err(Error::Conflict);
            }
        }
        let creation = if inventory.is_empty() {
            json_string(&(
                q.team,
                q.project,
                q.order,
                q.project_revision,
                &q.target,
                &q.capabilities,
                &q.input,
                q.model_profile,
                q.snapshot,
                &q.key,
            ))?
        } else {
            json_string(&(
                q.team,
                q.project,
                q.order,
                q.project_revision,
                &q.target,
                &q.capabilities,
                &q.input,
                q.model_profile,
                q.snapshot,
                &inventory,
                &q.key,
            ))?
        };
        let digest = if let Some(task) = &q.task {
            hash(json_string(&(creation, task))?.as_bytes())
        } else {
            hash(creation.as_bytes())
        };
        if let Some(r) =
            sqlx::query("SELECT * FROM android_scoped_runs WHERE creator_id=$1 AND creation_key=$2")
                .bind(actor.identity_id().to_string())
                .bind(&q.key)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
        {
            if r.try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != digest
            {
                return Err(Error::Conflict);
            }
            let value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
            if let Some(permit) = &permit {
                self.identity()
                    .validate_content_in_transaction(&mut tx, permit)
                    .await
                    .map_err(identity_error)?;
            } else {
                self.android_personal_permit(&mut tx, actor, q.project, ContentAction::Execute)
                    .await?;
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(value);
        }
        let scoped = self
            .identity()
            .session_organization_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?
            .is_some();
        let kind = q.target["kind"].as_str().ok_or_else(device_invalid)?;
        let candidates = match kind {
            "personal_device" => {
                let id = q.target["device_id"]
                    .as_str()
                    .and_then(|v| Uuid::parse_str(v).ok())
                    .ok_or_else(device_invalid)?;
                sqlx::query("SELECT * FROM android_devices WHERE id=$1 AND identity_id=$2 AND revoked=0 AND draining=0 AND (team_id=$3 OR (team_id IS NULL AND $4=0))").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(q.team.map(|t|t.to_string())).bind(i64::from(scoped)).fetch_all(&mut *tx).await.map_err(storage_error)?
            }
            "managed_windows_pool" => {
                let pool = q.target["pool_id"]
                    .as_str()
                    .and_then(|v| Uuid::parse_str(v).ok())
                    .ok_or_else(device_invalid)?;
                let rows=sqlx::query("SELECT * FROM android_devices WHERE team_id=$1 AND revoked=0 AND draining=0 ORDER BY id").bind(q.team.map(|t|t.to_string())).fetch_all(&mut *tx).await.map_err(storage_error)?;
                rows.into_iter()
                    .filter(|r| {
                        parse::<DeviceExecutorRegistration>(
                            &r.try_get::<String, _>("registration").unwrap_or_default(),
                        )
                        .is_ok_and(|v| v.kind == "managed_windows" && v.pool_id == Some(pool))
                    })
                    .collect()
            }
            _ => return Err(device_invalid()),
        };
        let mut selected = None;
        for d in candidates {
            let registration: DeviceExecutorRegistration = parse(
                &d.try_get::<String, _>("registration")
                    .map_err(storage_error)?,
            )?;
            if q.capabilities
                .iter()
                .all(|c| registration.capabilities.iter().any(|cap| &cap.name == c))
            {
                selected = Some(d);
                break;
            }
        }
        let d = selected.ok_or(Error::NotFound)?;
        let id = Uuid::new_v4();
        let executor = uuid(&d, "id")?;
        let session_revision:i64=sqlx::query_scalar("SELECT revision FROM identity_sessions WHERE id=$1 AND identity_id=$2 AND revoked=0 AND expires_at>$3").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(current).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let epoch: i64 = if let Some(team) = q.team {
            sqlx::query_scalar("SELECT authorization_epoch FROM identity_organizations WHERE id=$1 AND state='active'").bind(team.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?
        } else {
            0
        };
        let frozen = if files.is_empty() {
            q.snapshot
        } else {
            Some(Uuid::new_v4())
        };
        let spec = json!({"schema_version":2,"id":id,"thread_id":q.order,"project_id":q.project,"project":{"id":q.project,"revision":q.project_revision},"project_privacy":if q.team.is_some(){"team_managed"}else{"private_local"},"task":q.task,"creator_user_id":actor.identity_id(),"executor_target":q.target,"required_capabilities":q.capabilities,"input":q.input,"model_profile_id":q.model_profile,"snapshot_id":frozen,"idempotency_key":q.key,"created_at":device_iso(current)?});
        let mut initial_state = "queued";
        if q.team.is_none() {
            crate::android_private_snapshots::validate_files(
                &mut tx,
                actor.identity_id(),
                q.project,
                &q.input,
                current,
            )
            .await?;
            if let Some(snapshot) = q.snapshot {
                let ready:Option<i64>=sqlx::query_scalar("SELECT committed FROM android_private_snapshots WHERE id=$1 AND owner_id=$2 AND project_id=$3 AND deleted=0").bind(snapshot.to_string()).bind(actor.identity_id().to_string()).bind(q.project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
                if let Some(ready) = ready {
                    crate::android_private_snapshots::manifest(
                        &mut tx,
                        actor.identity_id(),
                        q.project,
                        snapshot,
                        false,
                        current,
                    )
                    .await?;
                    if ready == 0 {
                        initial_state = "waiting_for_snapshot";
                    }
                }
            }
        }
        let v = json!({"spec":spec,"state":initial_state,"revision":1,"etag":format!("{id}:1"),"assigned_executor_id":executor,"lease_expires_at":null,"started_at":null,"finished_at":null,"result":null,"error":null,"updated_at":device_iso(current)?});
        sqlx::query("INSERT INTO android_scoped_runs(id,team_id,project_id,order_id,creator_id,creator_session_id,creator_session_revision,authorization_epoch,project_revision,executor_id,executor_revision,creation_key,request_sha256,payload,state,revision,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$16,1,$15,$15)").bind(id.to_string()).bind(q.team.map(|t|t.to_string())).bind(q.project.to_string()).bind(q.order.to_string()).bind(actor.identity_id().to_string()).bind(actor.session_id().to_string()).bind(session_revision).bind(epoch).bind(q.project_revision).bind(executor.to_string()).bind(d.try_get::<i64,_>("execution_revision").map_err(storage_error)?).bind(q.key).bind(digest).bind(json_string(&v)?).bind(current).bind(initial_state).execute(&mut *tx).await.map_err(storage_error)?;
        if !files.is_empty() {
            let read = self
                .identity()
                .authorize_content_in_transaction(
                    &mut tx,
                    actor,
                    q.team.ok_or(Error::Forbidden)?,
                    q.project,
                    ContentAction::Read,
                )
                .await
                .map_err(identity_error)?;
            let snapshot = frozen.ok_or(Error::Conflict)?;
            let manifest = json!({"schema_version":2,"id":snapshot,"project_id":q.project,"total_bytes":total,"files":inventory,"encryption_key_id":"none","created_at":device_iso(current)?,"expires_at":null});
            sqlx::query("INSERT INTO android_scoped_snapshots(id,run_id,payload,committed) VALUES($1,$2,$3,1)").bind(snapshot.to_string()).bind(id.to_string()).bind(json_string(&manifest)?).execute(&mut *tx).await.map_err(storage_error)?;
            for (_, bytes) in &files {
                for chunk in bytes.chunks(4 * 1024 * 1024) {
                    use base64::Engine;
                    sqlx::query("INSERT INTO android_scoped_snapshot_chunks(manifest_id,digest,bytes_base64) VALUES($1,$2,$3) ON CONFLICT(manifest_id,digest) DO NOTHING").bind(snapshot.to_string()).bind(hash(chunk)).bind(base64::engine::general_purpose::STANDARD.encode(chunk)).execute(&mut *tx).await.map_err(storage_error)?;
                }
            }
            self.identity()
                .validate_content_in_transaction(&mut tx, &read)
                .await
                .map_err(identity_error)?;
        }
        if let Some(snapshot) = q.snapshot {
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_scoped_snapshots s JOIN android_scoped_runs r ON r.id=s.run_id WHERE s.id=$1 AND r.project_id=$2 AND s.committed=1").bind(snapshot.to_string()).bind(q.project.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if count != 1 {
                if q.team.is_some() {
                    return Err(Error::Forbidden);
                }
                crate::android_private_snapshots::manifest(
                    &mut tx,
                    actor.identity_id(),
                    q.project,
                    snapshot,
                    false,
                    current,
                )
                .await?;
            }
        }
        device_event(&mut tx, id, None, "run_created", &json!({}), current).await?;
        if let Some(permit) = &permit {
            self.identity()
                .validate_content_in_transaction(&mut tx, permit)
                .await
                .map_err(identity_error)?;
        } else {
            self.android_personal_permit(&mut tx, actor, q.project, ContentAction::Execute)
                .await?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_device_run(&self, actor: &Principal, id: Uuid) -> Result<Option<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let r = sqlx::query("SELECT * FROM android_scoped_runs WHERE id=$1")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let Some(r) = r else {
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        };
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Read).await?;
        let v = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(v))
    }
    pub async fn android_device_runs(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = self
            .identity()
            .authorize_content_in_transaction(&mut tx, actor, team, project, ContentAction::Read)
            .await
            .map_err(identity_error)?;
        let rows=sqlx::query("SELECT payload FROM android_scoped_runs WHERE team_id=$1 AND project_id=$2 ORDER BY created_at LIMIT 2000").bind(team.to_string()).bind(project.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let v = rows
            .iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<_>>>()?;
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_device_claim(&self, a: &AndroidAgentPrincipal) -> Result<Option<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let registration = agent_current(&mut tx, a, current).await?;
        let draining: i64 = sqlx::query_scalar("SELECT draining FROM android_devices WHERE id=$1")
            .bind(a.executor_id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_scoped_runs WHERE executor_id=$1 AND state IN('leased','running')").bind(a.executor_id.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if draining != 0 || active >= i64::from(registration.max_concurrent_runs) {
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        }
        let rows=sqlx::query("SELECT * FROM android_scoped_runs WHERE executor_id=$1 AND state='queued' ORDER BY created_at LIMIT 20").bind(a.executor_id.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        for r in rows {
            if r.try_get::<i64, _>("executor_revision")
                .map_err(storage_error)?
                != a.device_revision
                || device_run_authority(&mut tx, &r, current).await.is_err()
            {
                let mut v = updated(&r, "interrupted", current)?;
                v["error"] = json!({"code":"device_authority_changed","message":"Execution authority changed","retryable":false});
                save_run(&mut tx, &r, &v, "interrupted", current).await?;
                continue;
            }
            let token = Uuid::new_v4();
            let expiry = current + 30_000;
            let mut v = updated(&r, "running", current)?;
            v["started_at"] = json!(device_iso(current)?);
            v["lease_expires_at"] = json!(device_iso(expiry)?);
            save_run(&mut tx, &r, &v, "running", current).await?;
            sqlx::query("UPDATE android_scoped_runs SET lease_token=$1,credential_id=$2,credential_revision=$3,lease_expires_at=$4 WHERE id=$5 AND state='running'").bind(token.to_string()).bind(a.credential_id.to_string()).bind(a.credential_revision).bind(expiry).bind(r.try_get::<String,_>("id").map_err(storage_error)?).execute(&mut *tx).await.map_err(storage_error)?;
            let id = uuid(&r, "id")?;
            device_event(
                &mut tx,
                id,
                None,
                "run_leased",
                &json!({"executor_id":a.executor_id}),
                current,
            )
            .await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(Some(
                json!({"schema_version":2,"run":v,"lease_token":token,"lease_expires_at":device_iso(expiry)?}),
            ));
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(None)
    }
    pub async fn android_device_lease_heartbeat(
        &self,
        a: &AndroidAgentPrincipal,
        id: Uuid,
        token: Uuid,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = device_lease_current(&mut tx, a, id, token, current).await?;
        let expiry = current + 30_000;
        let mut v: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        v["lease_expires_at"] = json!(device_iso(expiry)?);
        sqlx::query("UPDATE android_scoped_runs SET lease_expires_at=$1,payload=$2 WHERE id=$3 AND lease_token=$4").bind(expiry).bind(json_string(&v)?).bind(id.to_string()).bind(token.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"schema_version":2,"run":v,"lease_token":token,"lease_expires_at":device_iso(expiry)?}),
        )
    }
    pub async fn android_device_append_event(
        &self,
        a: &AndroidAgentPrincipal,
        id: Uuid,
        token: Uuid,
        source: Option<Uuid>,
        kind: &str,
        payload: Value,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        device_lease_current(&mut tx, a, id, token, current).await?;
        let v = device_event(&mut tx, id, source, kind, &payload, current).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_device_finish(
        &self,
        a: &AndroidAgentPrincipal,
        id: Uuid,
        token: Uuid,
        success: bool,
        result: Value,
        manifest: Option<Uuid>,
    ) -> Result<Value> {
        self.android_device_finish_with_diff(a, id, token, success, result, manifest, Value::Null)
            .await
    }
    pub async fn android_device_finish_with_diff(
        &self,
        a: &AndroidAgentPrincipal,
        id: Uuid,
        token: Uuid,
        success: bool,
        mut result: Value,
        manifest: Option<Uuid>,
        diff: Value,
    ) -> Result<Value> {
        if json_string(&result)?.len() > 512 * 1024 {
            return Err(device_invalid());
        }
        if json_string(&diff)?.len() > 128 * 1024 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        agent_current(&mut tx, a, current).await?;
        let digest = if diff.is_null() {
            hash(json_string(&(success, &result, manifest))?.as_bytes())
        } else {
            hash(json_string(&(success, &result, manifest, &diff))?.as_bytes())
        };
        let existing = device_run_row(&mut tx, id).await?;
        if matches!(
            existing
                .try_get::<String, _>("state")
                .map_err(storage_error)?
                .as_str(),
            "completed" | "failed"
        ) {
            if !crate::android_private_schedules::validate_original_receipt(
                &mut tx,
                &existing,
                "personal_device",
                current,
            )
            .await?
            {
                device_run_authority(&mut tx, &existing, current).await?;
            }
            if uuid(&existing, "executor_id")? != a.executor_id
                || existing
                    .try_get::<i64, _>("executor_revision")
                    .map_err(storage_error)?
                    != a.device_revision
                || existing
                    .try_get::<Option<String>, _>("credential_id")
                    .map_err(storage_error)?
                    != Some(a.credential_id.to_string())
                || existing
                    .try_get::<Option<i64>, _>("credential_revision")
                    .map_err(storage_error)?
                    != Some(a.credential_revision)
                || existing
                    .try_get::<Option<String>, _>("lease_token")
                    .map_err(storage_error)?
                    != Some(token.to_string())
                || existing
                    .try_get::<Option<String>, _>("completion_sha256")
                    .map_err(storage_error)?
                    != Some(digest.clone())
            {
                return Err(Error::Conflict);
            }
            let v = parse(
                &existing
                    .try_get::<String, _>("payload")
                    .map_err(storage_error)?,
            )?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let r = device_lease_current(&mut tx, a, id, token, current).await?;
        if let Some(manifest) = manifest {
            let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_scoped_snapshots WHERE id=$1 AND run_id=$2 AND committed=1").bind(manifest.to_string()).bind(id.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if n != 1 {
                return Err(Error::Conflict);
            }
        }
        let payload: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let output = payload
            .pointer("/spec/input/output")
            .filter(|value| !value.is_null())
            .map(|value| serde_json::from_value::<StructuredOutput>(value.clone()))
            .transpose()
            .map_err(|_| device_invalid())?;
        let rejected_output = success
            && output.as_ref().is_some_and(|output| {
                result
                    .get("content")
                    .and_then(Value::as_str)
                    .is_none_or(|content| {
                        cowork_domain::structured_output::validate_result(output, content).is_err()
                    })
            });
        let accepted = success && !rejected_output;
        if accepted {
            if let Some(output) = &output {
                let verified = cowork_domain::structured_output::validate_result(
                    output,
                    result["content"].as_str().ok_or_else(device_invalid)?,
                )?;
                result["structured_output"] = verified;
                result["output_contract_sha256"] = json!(cowork_domain::canonical_hash(output));
            }
        }
        let state = if accepted { "completed" } else { "failed" };
        let mut v = updated(&r, state, current)?;
        v[if accepted { "result" } else { "error" }] = if rejected_output {
            json!({"code":"structured_output_mismatch","received_result":result})
        } else {
            result.clone()
        };
        v["result_snapshot_manifest_id"] = json!(manifest);
        v["result_diff_summary"] = diff;
        v["finished_at"] = json!(device_iso(current)?);
        v["lease_expires_at"] = Value::Null;
        save_run(&mut tx, &r, &v, state, current).await?;
        sqlx::query("UPDATE android_scoped_runs SET completion_sha256=$1 WHERE id=$2")
            .bind(digest)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        device_event(
            &mut tx,
            id,
            None,
            if accepted {
                "run_completed"
            } else {
                "run_failed"
            },
            &result,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_device_cancel(&self, actor: &Principal, id: Uuid) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let current = device_writer(&mut tx).await?;
        let r = device_run_row(&mut tx, id).await?;
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Execute).await?;
        let state = r.try_get::<String, _>("state").map_err(storage_error)?;
        let mut v: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if matches!(state.as_str(), "queued" | "leased" | "running") {
            let next = if state == "queued" {
                "canceled"
            } else {
                "interrupted"
            };
            v = updated(&r, next, current)?;
            v["error"] = json!({"code":"device_stop_requested","message":"Authority revoked. Physical device stop is not yet confirmed.","retryable":false});
            save_run(&mut tx, &r, &v, next, current).await?;
            device_event(
                &mut tx,
                id,
                None,
                "run_canceled",
                &json!({"physical_stop_confirmed":false}),
                current,
            )
            .await?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_device_expire_leases(&self) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let rows=sqlx::query("SELECT * FROM android_scoped_runs WHERE state IN('leased','running') AND lease_expires_at<=$1 LIMIT 100").bind(current).fetch_all(&mut *tx).await.map_err(storage_error)?;
        for r in rows {
            let mut v = updated(&r, "interrupted", current)?;
            v["error"] = json!({"code":"device_lease_expired","message":"Device execution outcome is unknown","retryable":false});
            save_run(&mut tx, &r, &v, "interrupted", current).await?;
            device_event(
                &mut tx,
                uuid(&r, "id")?,
                None,
                "run_interrupted",
                &json!({}),
                current,
            )
            .await?;
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_device_events(
        &self,
        actor: &Principal,
        id: Uuid,
        after: i64,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let r = device_run_row(&mut tx, id).await?;
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Read).await?;
        let rows=sqlx::query("SELECT payload FROM android_scoped_events WHERE run_id=$1 AND sequence>$2 ORDER BY sequence LIMIT 1000").bind(id.to_string()).bind(after.max(0)).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let v = rows
            .iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
}
