//! Private server queue: real session authority, immutable provider admission and no unknown-effect replay.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use crate::android_personal_provider;
use cowork_identity::{ContentAction, Principal};
use cowork_secrets::{CredentialScope, KeyStore, SealedCredential, open_bearer};
use sqlx::{Any, Transaction, any::AnyRow};
use team_service::identity_error;
use zeroize::Zeroizing;
pub struct PrivateServerLease {
    pub run: Uuid,
    pub lease: Uuid,
    pub value: Value,
}
pub(crate) fn id(r: &AnyRow, k: &str) -> Result<Uuid> {
    Uuid::parse_str(&r.try_get::<String, _>(k).map_err(storage_error)?).map_err(storage_error)
}
pub(crate) async fn row(tx: &mut Transaction<'_, Any>, run: Uuid) -> Result<AnyRow> {
    sqlx::query("SELECT * FROM android_private_server_runs WHERE id=$1")
        .bind(run.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)
}
pub(crate) async fn event(
    tx: &mut Transaction<'_, Any>,
    run: Uuid,
    kind: &str,
    payload: &Value,
    current: i64,
) -> Result<()> {
    let sequence: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(sequence),0)+1 FROM android_private_server_events WHERE run_id=$1",
    )
    .bind(run.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    let v = json!({"schema_version":2,"run_id":run,"sequence":sequence,"event_id":Uuid::new_v4(),"event_type":kind,"payload":payload,"created_at":device_iso(current)?});
    sqlx::query(
        "INSERT INTO android_private_server_events(run_id,sequence,payload) VALUES($1,$2,$3)",
    )
    .bind(run.to_string())
    .bind(sequence)
    .bind(json_string(&v)?)
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?;
    Ok(())
}
pub(crate) async fn save(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    state: &str,
    result: Option<&Value>,
    error: Option<&Value>,
    current: i64,
) -> Result<Value> {
    let mut v: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    v["state"] = json!(state);
    v["revision"] = json!(v["revision"].as_i64().ok_or_else(device_invalid)? + 1);
    v["etag"] = json!(format!("{}:{}", id(r, "id")?, v["revision"]));
    v["updated_at"] = json!(device_iso(current)?);
    if state == "running" && v["started_at"].is_null() {
        v["started_at"] = json!(device_iso(current)?)
    }
    if matches!(state, "completed" | "failed" | "canceled" | "interrupted") {
        v["finished_at"] = json!(device_iso(current)?)
    }
    if let Some(result) = result {
        v["result"] = result.clone()
    }
    if let Some(error) = error {
        v["error"] = error.clone()
    }
    sqlx::query("UPDATE android_private_server_runs SET payload=$1,state=$2 WHERE id=$3")
        .bind(json_string(&v)?)
        .bind(state)
        .bind(id(r, "id")?.to_string())
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    event(
        tx,
        id(r, "id")?,
        "run_state_changed",
        &json!({"state":state}),
        current,
    )
    .await?;
    Ok(v)
}
pub(crate) async fn authority(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    current: i64,
) -> Result<Value> {
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM identity_subjects i JOIN identity_sessions s ON s.identity_id=i.id JOIN android_personal_projects p ON p.owner_id=i.id JOIN android_personal_threads t ON t.owner_id=i.id AND t.project_id=p.id WHERE i.id=$1 AND i.active=1 AND s.id=$2 AND s.revision=$3 AND s.revoked=0 AND s.expires_at>$4 AND NOT EXISTS(SELECT 1 FROM identity_session_scopes ss WHERE ss.session_id=s.id) AND p.id=$5 AND p.revision=$6 AND p.deleted=0 AND t.id=$7 AND t.deleted=0").bind(id(r,"owner_id")?.to_string()).bind(id(r,"session_id")?.to_string()).bind(r.try_get::<i64,_>("session_revision").map_err(storage_error)?).bind(current).bind(id(r,"project_id")?.to_string()).bind(r.try_get::<i64,_>("project_revision").map_err(storage_error)?).bind(id(r,"thread_id")?.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if count != 1 {
        return Err(Error::Forbidden);
    }
    let v: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if let Some(task) = v.pointer("/spec/task").filter(|v| !v.is_null()) {
        let tid = crate::android_personal::wire_uuid(task, "id")?;
        let revision = task["revision"]
            .as_i64()
            .filter(|v| *v > 0)
            .ok_or_else(device_invalid)?;
        let stored:String=sqlx::query_scalar("SELECT payload FROM android_task_versions WHERE id=$1 AND revision=$2 AND project_id=$3 AND released=1 AND deleted=0").bind(tid.to_string()).bind(revision).bind(id(r,"project_id")?.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let task: Value = parse(&stored)?;
        if v.pointer("/spec/input/task_instructions") != task.get("instructions")
            || v.pointer("/spec/input/task_config") != task.get("config")
        {
            return Err(Error::Forbidden);
        }
    }
    let profile =
        android_personal_provider::current(tx, id(r, "owner_id")?, id(r, "provider_id")?).await?;
    crate::android_private_snapshots::validate_files(
        tx,
        id(r, "owner_id")?,
        id(r, "project_id")?,
        &v["spec"]["input"],
        current,
    )
    .await?;
    crate::android_private_mcp::validate_bindings(
        tx,
        id(r, "owner_id")?,
        id(r, "project_id")?,
        &v["spec"]["input"],
    )
    .await?;
    crate::android_private_github::validate_binding(tx,id(r,"owner_id")?,id(r,"project_id")?,&v["spec"]["input"]).await?;
    if android_personal_provider::binding(&profile)? != v["spec"]["input"]["provider_binding"] {
        return Err(Error::Forbidden);
    }
    crate::actor_model_registry::check_reference(
        tx,
        id(r, "owner_id")?,
        id(r, "project_id")?,
        &profile,
        &v["spec"]["input"]["model_binding"],
    )
    .await?;
    Ok(profile)
}
pub(crate) async fn leased(
    tx: &mut Transaction<'_, Any>,
    run: Uuid,
    lease: Uuid,
    current: i64,
) -> Result<AnyRow> {
    let r = row(tx, run).await?;
    if r.try_get::<Option<String>, _>("lease_id")
        .map_err(storage_error)?
        != Some(lease.to_string())
        || r.try_get::<i64, _>("lease_until").map_err(storage_error)? <= current
        || !matches!(
            r.try_get::<String, _>("state")
                .map_err(storage_error)?
                .as_str(),
            "running" | "waiting_for_approval" | "waiting_for_input"
        )
    {
        return Err(Error::Forbidden);
    }
    authority(tx, &r, current).await?;
    Ok(r)
}
pub(crate) async fn snapshot_ready(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    snapshot: Uuid,
    current: i64,
) -> Result<()> {
    let rows=sqlx::query("SELECT * FROM android_private_server_runs WHERE owner_id=$1 AND project_id=$2 AND snapshot_id=$3 AND state='waiting_for_snapshot'").bind(owner.to_string()).bind(project.to_string()).bind(snapshot.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    for r in rows {
        save(tx, &r, "queued", None, None, current).await?;
    }
    let device_rows=sqlx::query("SELECT * FROM android_scoped_runs WHERE creator_id=$1 AND team_id IS NULL AND project_id=$2 AND state='waiting_for_snapshot'").bind(owner.to_string()).bind(project.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    for r in device_rows {
        let mut v: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if v["spec"]["snapshot_id"] != json!(snapshot) {
            continue;
        }
        let revision = r.try_get::<i64, _>("revision").map_err(storage_error)? + 1;
        v["revision"] = json!(revision);
        v["etag"] = json!(format!("{}:{revision}", id(&r, "id")?));
        v["state"] = json!("queued");
        v["updated_at"] = json!(device_iso(current)?);
        crate::android_device_runs::save_run(tx, &r, &v, "queued", current).await?;
        crate::android_device_runs::device_event(
            tx,
            id(&r, "id")?,
            None,
            "run_queued",
            &json!({"snapshot_id":snapshot}),
            current,
        )
        .await?;
    }
    Ok(())
}
impl Store {
    pub async fn android_private_server_create(
        &self,
        actor: &Principal,
        request: &Value,
    ) -> Result<Value> {
        let project = crate::android_personal::wire_uuid(request, "project_id")?;
        let thread = crate::android_personal::wire_uuid(request, "thread_id")?;
        let provider = crate::android_personal::wire_uuid(request, "model_profile_id")?;
        let key = request["idempotency_key"]
            .as_str()
            .ok_or_else(device_invalid)?;
        valid_text(key, 200)?;
        let request_sha = hash(json_string(request)?.as_bytes());
        if request["project_privacy"] != "private_local"
            || request["executor_target"]["kind"] != "server_linux"
            || request["required_capabilities"]
                .as_array()
                .is_none_or(|a| !a.is_empty())
        {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let p = self
            .android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        if let Some(r)=sqlx::query("SELECT request_sha,initial_receipt FROM android_private_server_runs WHERE owner_id=$1 AND creation_key=$2").bind(actor.identity_id().to_string()).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)?{if r.try_get::<String,_>("request_sha").map_err(storage_error)?!=request_sha{return Err(Error::Conflict)}let receipt=parse(&r.try_get::<String,_>("initial_receipt").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(receipt)}
        if p["revision"] != request["project_revision"] {
            return Err(Error::Conflict);
        }
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_threads WHERE id=$1 AND owner_id=$2 AND project_id=$3 AND deleted=0").bind(thread.to_string()).bind(actor.identity_id().to_string()).bind(project.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1 {
            return Err(Error::Forbidden);
        }
        let profile =
            android_personal_provider::current(&mut tx, actor.identity_id(), provider).await?;
        if android_personal_provider::binding(&profile)? != request["input"]["provider_binding"] {
            return Err(Error::Forbidden);
        }
        if profile["has_secret"] != true && profile["model_defaults"]["auth_mode"] != "none" {
            return Err(Error::Forbidden);
        }
        crate::actor_model_registry::check_reference(
            &mut tx,
            actor.identity_id(),
            project,
            &profile,
            &request["input"]["model_binding"],
        )
        .await?;
        let snapshot = request["snapshot_id"]
            .as_str()
            .map(|s| Uuid::parse_str(s).map_err(|_| device_invalid()))
            .transpose()?;
        let state = if let Some(snapshot) = snapshot {
            let _ = crate::android_private_snapshots::manifest(
                &mut tx,
                actor.identity_id(),
                project,
                snapshot,
                false,
                current,
            )
            .await?;
            let ready: i64 =
                sqlx::query_scalar("SELECT committed FROM android_private_snapshots WHERE id=$1")
                    .bind(snapshot.to_string())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            if ready == 1 {
                "queued"
            } else {
                "waiting_for_snapshot"
            }
        } else {
            "queued"
        };
        crate::android_private_snapshots::validate_files(
            &mut tx,
            actor.identity_id(),
            project,
            &request["input"],
            current,
        )
        .await?;
        let revision:i64=sqlx::query_scalar("SELECT revision FROM identity_sessions WHERE id=$1 AND identity_id=$2 AND revoked=0 AND expires_at>$3").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(current).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let run = Uuid::new_v4();
        let mut spec = request.clone();
        spec["schema_version"] = json!(2);
        spec["id"] = json!(run);
        spec["project"] = json!({"id":project,"revision":p["revision"]});
        spec["creator_user_id"] = json!(actor.identity_id());
        spec["created_at"] = json!(device_iso(current)?);
        let v = json!({"spec":spec,"state":state,"revision":1,"etag":format!("{run}:1"),"assigned_executor_id":null,"lease_expires_at":null,"started_at":null,"finished_at":null,"result":null,"error":null,"updated_at":device_iso(current)?});
        sqlx::query("INSERT INTO android_private_server_runs(id,owner_id,session_id,session_revision,project_id,project_revision,thread_id,provider_id,snapshot_id,creation_key,request_sha,initial_receipt,payload,state,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$12,$13,$14)").bind(run.to_string()).bind(actor.identity_id().to_string()).bind(actor.session_id().to_string()).bind(revision).bind(project.to_string()).bind(p["revision"].as_i64().ok_or_else(device_invalid)?).bind(thread.to_string()).bind(provider.to_string()).bind(snapshot.map(|s|s.to_string())).bind(key).bind(request_sha).bind(json_string(&v)?).bind(state).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        let r = row(&mut tx, run).await?;
        authority(&mut tx, &r, current).await?;
        event(
            &mut tx,
            run,
            "run_created",
            &json!({"state":state}),
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_private_server_run(
        &self,
        actor: &Principal,
        run: Uuid,
    ) -> Result<Option<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let r = sqlx::query("SELECT * FROM android_private_server_runs WHERE id=$1")
            .bind(run.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let Some(r) = r else { return Ok(None) };
        if id(&r, "owner_id")? != actor.identity_id() {
            return Err(Error::Forbidden);
        }
        self.android_personal_permit(&mut tx, actor, id(&r, "project_id")?, ContentAction::Read)
            .await?;
        let v = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(v))
    }
    pub async fn android_private_server_cancel(
        &self,
        actor: &Principal,
        run: Uuid,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        if id(&r, "owner_id")? != actor.identity_id() {
            return Err(Error::Forbidden);
        }
        self.android_personal_permit(
            &mut tx,
            actor,
            id(&r, "project_id")?,
            ContentAction::Execute,
        )
        .await?;
        let state = r.try_get::<String, _>("state").map_err(storage_error)?;
        let v = if matches!(
            state.as_str(),
            "queued"
                | "waiting_for_snapshot"
                | "running"
                | "waiting_for_approval"
                | "waiting_for_input"
        ) {
            let unknown:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_loop_steps WHERE run_id=$1 AND state!='received'").bind(run.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            let admitted = unknown > 0
                || r.try_get::<Option<String>, _>("effect_intent")
                    .map_err(storage_error)?
                    .is_some();
            save(&mut tx,&r,if admitted{"interrupted"}else{"canceled"},None,Some(&json!({"code":if admitted{"private_effect_uncertain"}else{"user_cancelled"},"retryable":false})),current).await?
        } else {
            parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_private_server_events(
        &self,
        actor: &Principal,
        run: Uuid,
        after: i64,
    ) -> Result<Vec<Value>> {
        self.android_private_server_run(actor, run)
            .await?
            .ok_or(Error::NotFound)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let r = row(&mut tx, run).await?;
        self.android_personal_permit(&mut tx, actor, id(&r, "project_id")?, ContentAction::Read)
            .await?;
        if id(&r, "owner_id")? != actor.identity_id() {
            return Err(Error::Forbidden);
        }
        let rows:Vec<String>=sqlx::query_scalar("SELECT payload FROM android_private_server_events WHERE run_id=$1 AND sequence>$2 ORDER BY sequence LIMIT 500").bind(run.to_string()).bind(after.max(0)).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let events = rows
            .iter()
            .map(|s| parse(s))
            .collect::<Result<Vec<Value>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(events)
    }
    pub async fn android_private_server_claim(&self) -> Result<Option<PrivateServerLease>> {
        self.android_private_server_claim_for_runner(None).await
    }
    pub async fn android_private_server_claim_for_runner(
        &self,
        runner: Option<&Value>,
    ) -> Result<Option<PrivateServerLease>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let expired = sqlx::query(
            "SELECT * FROM android_private_server_runs WHERE state IN('running','waiting_for_approval','waiting_for_input') AND lease_until<=$1",
        )
        .bind(current)
        .fetch_all(&mut *tx)
        .await
        .map_err(storage_error)?;
        for r in expired {
            save(
                &mut tx,
                &r,
                "interrupted",
                None,
                Some(&json!({"code":"private_worker_lost","retryable":false})),
                current,
            )
            .await?;
        }
        let rows=sqlx::query("SELECT * FROM android_private_server_runs WHERE state='queued' ORDER BY created_at LIMIT 100").fetch_all(&mut *tx).await.map_err(storage_error)?;
        for r in rows {
            let frozen: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let expected = &frozen["spec"]["input"]["frozen_private_runner"];
            if !expected.is_null() && runner != Some(expected) {
                continue;
            }
            if authority(&mut tx, &r, current).await.is_err() {
                save(
                    &mut tx,
                    &r,
                    "failed",
                    None,
                    Some(&json!({"code":"authority_changed","retryable":false})),
                    current,
                )
                .await?;
                continue;
            }
            if let Some(snapshot) = r
                .try_get::<Option<String>, _>("snapshot_id")
                .map_err(storage_error)?
            {
                crate::android_private_snapshots::manifest(
                    &mut tx,
                    id(&r, "owner_id")?,
                    id(&r, "project_id")?,
                    Uuid::parse_str(&snapshot).map_err(storage_error)?,
                    true,
                    current,
                )
                .await?;
            }
            let run = id(&r, "id")?;
            let lease = Uuid::new_v4();
            sqlx::query("UPDATE android_private_server_runs SET lease_id=$1,lease_until=$2 WHERE id=$3 AND state='queued'").bind(lease.to_string()).bind(current+30_000).bind(run.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
            let value = save(&mut tx, &r, "running", None, None, current).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(Some(PrivateServerLease { run, lease, value }));
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(None)
    }
    pub async fn android_private_server_dispatch(
        &self,
        run: Uuid,
        lease: Uuid,
        intent: &Value,
        keys: &dyn KeyStore,
    ) -> Result<Option<Zeroizing<String>>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = leased(&mut tx, run, lease, current).await?;
        let profile = authority(&mut tx, &r, current).await?;
        if r.try_get::<Option<String>, _>("effect_intent")
            .map_err(storage_error)?
            .is_some()
        {
            return Err(Error::UncertainEffect);
        }
        if intent["provider_sha256"] != json!(cowork_domain::canonical_hash(&profile)) {
            return Err(Error::Forbidden);
        }
        let scope = android_personal_provider::credential_scope(&profile)?;
        let stored:Option<(String,String,i64)>=sqlx::query_as("SELECT scope,sealed,profile_revision FROM android_personal_provider_secrets WHERE profile_id=$1").bind(id(&r,"provider_id")?.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let secret = if let Some((stored, sealed, rev)) = stored {
            if parse::<CredentialScope>(&stored)? != scope || rev != scope.revision {
                return Err(Error::Forbidden);
            }
            if profile["model_defaults"]["auth_mode"] == "none" {
                None
            } else {
                Some(
                    open_bearer(keys, &scope, &parse::<SealedCredential>(&sealed)?)
                        .map_err(|_| Error::NativeKeyUnavailable)?,
                )
            }
        } else {
            if profile["model_defaults"]["auth_mode"] != "none" || profile["has_secret"] == true {
                return Err(Error::Forbidden);
            }
            None
        };
        authority(&mut tx, &r, current).await?;
        sqlx::query("UPDATE android_private_server_runs SET effect_intent=$1 WHERE id=$2 AND effect_intent IS NULL").bind(json_string(intent)?).bind(run.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(secret)
    }
    pub async fn android_private_server_renew(&self, run: Uuid, lease: Uuid) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let _ = leased(&mut tx, run, lease, current).await?;
        sqlx::query(
            "UPDATE android_private_server_runs SET lease_until=$1 WHERE id=$2 AND lease_id=$3",
        )
        .bind(current + 30_000)
        .bind(run.to_string())
        .bind(lease.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    /// Actual received bytes survive lost authority; this receipt never grants a resend.
    pub async fn android_private_server_received(
        &self,
        run: Uuid,
        lease: Uuid,
        received: &Value,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        if r.try_get::<Option<String>, _>("lease_id")
            .map_err(storage_error)?
            != Some(lease.to_string())
            || r.try_get::<Option<String>, _>("effect_intent")
                .map_err(storage_error)?
                .is_none()
        {
            return Err(Error::Forbidden);
        }
        let encoded = json_string(received)?;
        if encoded.len() > 2 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        let previous = r
            .try_get::<Option<String>, _>("effect_received")
            .map_err(storage_error)?;
        if previous.as_ref().is_some_and(|old| old != &encoded) {
            return Err(Error::Conflict);
        }
        sqlx::query("UPDATE android_private_server_runs SET effect_received=$1 WHERE id=$2 AND effect_received IS NULL").bind(encoded).bind(run.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_server_finish(
        &self,
        run: Uuid,
        lease: Uuid,
        result: Option<&Value>,
        error: Option<&Value>,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        if r.try_get::<Option<String>, _>("lease_id")
            .map_err(storage_error)?
            != Some(lease.to_string())
        {
            return Err(Error::Forbidden);
        }
        let state = r.try_get::<String, _>("state").map_err(storage_error)?;
        if state != "running" {
            let v = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let authorized = leased(&mut tx, run, lease, current).await.is_ok();
        let admitted = r
            .try_get::<Option<String>, _>("effect_intent")
            .map_err(storage_error)?
            .is_some();
        let received: Option<Value> = r
            .try_get::<Option<String>, _>("effect_received")
            .map_err(storage_error)?
            .map(|s| parse(&s))
            .transpose()?;
        let confirmed = received.as_ref().is_some_and(|v| {
            v["uncertain"] != true && v["wire_evidence"]["completed_stream"] == true
        });
        if let (Some(result), Some(received)) = (result, received.as_ref()) {
            if result["content"] != received["content"] || result["usage"] != received["usage"] {
                return Err(Error::Forbidden);
            }
        }
        let state = if result.is_some() && authorized && confirmed {
            "completed"
        } else if admitted && (!confirmed || !authorized) {
            "interrupted"
        } else {
            "failed"
        };
        let v = save(
            &mut tx,
            &r,
            state,
            if state == "completed" { result } else { None },
            error,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_private_server_context_files(
        &self,
        run: Uuid,
        lease: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = leased(&mut tx, run, lease, current).await?;
        let mut values = Vec::new();
        let mut total = 0usize;
        if let Some(snapshot) = r
            .try_get::<Option<String>, _>("snapshot_id")
            .map_err(storage_error)?
        {
            let snapshot = Uuid::parse_str(&snapshot).map_err(storage_error)?;
            let manifest = crate::android_private_snapshots::manifest(
                &mut tx,
                id(&r, "owner_id")?,
                id(&r, "project_id")?,
                snapshot,
                true,
                current,
            )
            .await?;
            for f in manifest["files"].as_array().ok_or_else(device_invalid)? {
                let path = f["path"].as_str().ok_or_else(device_invalid)?;
                let size = f["size"].as_u64().ok_or_else(device_invalid)?;
                let mut item = json!({"path":path,"size":size,"chunks":f["chunks"]});
                if size <= 65536 && total + (size as usize) <= 65536 {
                    let bytes = crate::android_private_snapshots::file_bytes(
                        &mut tx,
                        id(&r, "owner_id")?,
                        id(&r, "project_id")?,
                        snapshot,
                        path,
                        keys,
                        current,
                    )
                    .await?;
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        item["content"] = json!(text);
                        total += bytes.len();
                    }
                }
                values.push(item);
            }
        }
        leased(&mut tx, run, lease, current).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_private_server_profile(&self, run: Uuid, lease: Uuid) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = leased(&mut tx, run, lease, current).await?;
        let profile = authority(&mut tx, &r, current).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(profile)
    }
}
