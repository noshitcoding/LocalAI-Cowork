//! Actor-only recurring grants. Occurrences mint run identities, never login sessions.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use cowork_identity::{ContentAction, Principal};
use cowork_secrets::{KeyId, KeyStore};
use serde::{Deserialize, Serialize};
use sqlx::{Any, Transaction, any::AnyRow};
use team_service::identity_error;
pub const KEY_PREFIX: &str = "actor-schedule:";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    pub owner: Uuid,
    pub identity_revision: i64,
    pub session: Uuid,
    pub session_revision: i64,
    pub policy_revision: i64,
    pub project: Uuid,
    pub project_revision: i64,
    pub thread: Uuid,
    pub factor_pin: Value,
    pub task: Value,
    pub target: Value,
    pub input: Value,
    pub profile: Option<Uuid>,
    pub snapshot: Option<Uuid>,
    pub capabilities: Vec<String>,
    pub runner: Option<Value>,
    pub device_revision: Option<i64>,
    pub device_sha256: Option<String>,
}
pub struct PreparedSchedule {
    source: Source,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScheduledRun {
    pub owner: Uuid,
    pub session: Uuid,
    pub session_revision: i64,
    pub project: Uuid,
    pub project_revision: i64,
    pub thread: Uuid,
    pub profile: Option<Uuid>,
    pub snapshot: Option<Uuid>,
    pub target: Value,
    pub input: Value,
    pub task: Value,
    pub capabilities: Vec<String>,
    pub run: Uuid,
    pub key: String,
}
fn one(n: u64) -> Result<()> {
    if n == 1 { Ok(()) } else { Err(Error::Conflict) }
}
fn uuid(r: &AnyRow, k: &str) -> Result<Uuid> {
    Uuid::parse_str(&r.try_get::<String, _>(k).map_err(storage_error)?).map_err(storage_error)
}
fn operation(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 200 || key.chars().any(char::is_control) {
        Err(device_invalid())
    } else {
        Ok(())
    }
}
fn digest(purpose: &str, value: &Value) -> String {
    canonical_hash(
        &json!({"domain":"actor-schedule-original/v1","purpose":purpose,"request":value}),
    )
}
fn ids(input: &Value, key: &str) -> Result<Vec<Uuid>> {
    let Some(v) = input.get(key).or_else(|| {
        input.get("task_config").and_then(|v| {
            v.get(key)
                .or_else(|| v.get("sync_metadata").and_then(|v| v.get(key)))
        })
    }) else {
        return Ok(Vec::new());
    };
    let array = v
        .as_array()
        .filter(|v| v.len() <= 32)
        .ok_or_else(device_invalid)?;
    let mut result = Vec::new();
    for v in array {
        let id = Uuid::parse_str(v.as_str().ok_or_else(device_invalid)?)
            .map_err(|_| device_invalid())?;
        if id.is_nil() || result.contains(&id) {
            return Err(device_invalid());
        }
        result.push(id);
    }
    Ok(result)
}
fn credential_fields(v: &Value) -> bool {
    match v {
        Value::Object(o) => o.iter().any(|(k, v)| {
            matches!(
                k.to_ascii_lowercase().as_str(),
                "api_key"
                    | "password"
                    | "access_token"
                    | "refresh_token"
                    | "authorization"
                    | "bearer"
                    | "credential"
                    | "secret"
            ) || credential_fields(v)
        }),
        Value::Array(a) => a.iter().any(credential_fields),
        _ => false,
    }
}
async fn record(tx: &mut Transaction<'_, Any>, owner: Uuid, id: Uuid) -> Result<AnyRow> {
    sqlx::query("SELECT * FROM android_private_schedules WHERE id=$1 AND owner_id=$2")
        .bind(id.to_string())
        .bind(owner.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)
}
fn value(r: &AnyRow) -> Result<AndroidScheduleRecord> {
    let v: AndroidScheduleRecord =
        parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if v.id != uuid(r, "id")?
        || v.project_id != uuid(r, "project_id")?
        || v.thread_id != uuid(r, "thread_id")?
        || v.revision != r.try_get::<i64, _>("revision").map_err(storage_error)?
        || v.enabled != (r.try_get::<i64, _>("enabled").map_err(storage_error)? == 1)
    {
        return Err(Error::Forbidden);
    }
    Ok(v)
}
async fn saved(tx: &mut Transaction<'_, Any>, v: &AndroidScheduleRecord) -> Result<()> {
    let due = v
        .next_run_at
        .as_ref()
        .map(|v| {
            chrono::DateTime::parse_from_rfc3339(v)
                .map(|v| v.timestamp_millis())
                .map_err(storage_error)
        })
        .transpose()?;
    one(sqlx::query("UPDATE android_private_schedules SET revision=$1,enabled=$2,deleted=$3,next_run=$4,payload=$5 WHERE id=$6 AND project_id=$7 AND thread_id=$8").bind(v.revision).bind(i64::from(v.enabled)).bind(i64::from(v.deleted_at.is_some())).bind(due).bind(json_string(v)?).bind(v.id.to_string()).bind(v.project_id.to_string()).bind(v.thread_id.to_string()).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
    Ok(())
}
async fn receipt(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    key: &str,
    purpose: &str,
    sha: &str,
) -> Result<Option<Value>> {
    operation(key)?;
    let r=sqlx::query("SELECT purpose,request_sha256,receipt FROM android_private_schedule_operations WHERE owner_id=$1 AND operation_key=$2").bind(owner.to_string()).bind(key).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    match r {
        None => Ok(None),
        Some(r) => {
            if r.try_get::<String, _>("purpose").map_err(storage_error)? != purpose
                || r.try_get::<String, _>("request_sha256")
                    .map_err(storage_error)?
                    != sha
            {
                return Err(Error::Conflict);
            }
            Ok(Some(parse(
                &r.try_get::<String, _>("receipt").map_err(storage_error)?,
            )?))
        }
    }
}
async fn remember(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    key: &str,
    purpose: &str,
    sha: &str,
    v: &Value,
    current: i64,
) -> Result<()> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM android_private_schedule_operations WHERE owner_id=$1",
    )
    .bind(owner.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if n >= 10000 || json_string(v)?.len() > 512 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    sqlx::query("INSERT INTO android_private_schedule_operations(owner_id,operation_key,purpose,request_sha256,receipt,created_at) VALUES($1,$2,$3,$4,$5,$6)").bind(owner.to_string()).bind(key).bind(purpose).bind(sha).bind(json_string(v)?).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
async fn policy(tx: &mut Transaction<'_, Any>) -> Result<i64> {
    sqlx::query_scalar("SELECT revision FROM android_auth_security_policy WHERE id=1")
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)
}
async fn check_source(tx: &mut Transaction<'_, Any>, s: &Source, current: i64) -> Result<()> {
    let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM identity_subjects i JOIN android_personal_projects p ON p.owner_id=i.id JOIN android_personal_threads t ON t.owner_id=i.id AND t.project_id=p.id WHERE i.id=$1 AND i.active=1 AND i.revision=$2 AND p.id=$3 AND p.revision=$4 AND p.deleted=0 AND t.id=$5 AND t.deleted=0").bind(s.owner.to_string()).bind(s.identity_revision).bind(s.project.to_string()).bind(s.project_revision).bind(s.thread.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if n != 1 || policy(tx).await? != s.policy_revision {
        return Err(Error::Forbidden);
    }
    if s.factor_pin["method"] == "totp" || s.factor_pin["method"] == "recovery" {
        let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_totp WHERE owner_id=$1 AND enabled=1 AND revision=$2").bind(s.owner.to_string()).bind(s.factor_pin["revision"].as_i64().ok_or(Error::Forbidden)?).fetch_one(&mut **tx).await.map_err(storage_error)?;
        if n != 1 {
            return Err(Error::Forbidden);
        }
    }
    if s.factor_pin["method"] == "passkey" {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_passkeys WHERE identity_id=$1 AND id=$2 AND revision>=$3",
        )
        .bind(s.owner.to_string())
        .bind(
            s.factor_pin["passkey_id"]
                .as_str()
                .ok_or(Error::Forbidden)?,
        )
        .bind(s.factor_pin["revision"].as_i64().ok_or(Error::Forbidden)?)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
        if n != 1 {
            return Err(Error::Forbidden);
        }
    }
    let task_id = crate::android_personal::wire_uuid(&s.task, "id")?;
    let revision = s.task["revision"].as_i64().ok_or_else(device_invalid)?;
    let raw:String=sqlx::query_scalar("SELECT payload FROM android_task_versions WHERE id=$1 AND revision=$2 AND project_id=$3 AND released=1 AND deleted=0").bind(task_id.to_string()).bind(revision).bind(s.project.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let task: Value = parse(&raw)?;
    if s.input["task_instructions"] != task["instructions"]
        || s.input["task_config"] != task["config"]
    {
        return Err(Error::Forbidden);
    }
    if let Some(profile) = s.profile {
        let p = crate::android_personal_provider::current(tx, s.owner, profile).await?;
        if crate::android_personal_provider::binding(&p)? != s.input["provider_binding"] {
            return Err(Error::Forbidden);
        }
        crate::actor_model_registry::check_reference(
            tx,
            s.owner,
            s.project,
            &p,
            &s.input["model_binding"],
        )
        .await?;
    }
    crate::android_private_snapshots::validate_files(tx, s.owner, s.project, &s.input, current)
        .await?;
    if let Some(snapshot) = s.snapshot {
        crate::android_private_snapshots::manifest(tx, s.owner, s.project, snapshot, true, current)
            .await?;
    }
    if s.target["kind"]=="server_linux" {
        crate::android_server_capabilities::validate_private(tx,s.owner,s.project,&json!({"input":s.input,"required_capabilities":s.capabilities})).await?;
    }
    crate::android_private_mcp::validate_bindings(tx, s.owner, s.project, &s.input).await?;
    crate::android_private_github::validate_binding(tx, s.owner, s.project, &s.input).await?;
    if let Some(context) = s
        .input
        .get("frozen_runtime_context")
        .and_then(Value::as_object)
    {
        for (kind, items) in context {
            for item in items.as_array().ok_or_else(device_invalid)? {
                let id = crate::android_personal::wire_uuid(item, "entity_id")?;
                let row=sqlx::query("SELECT revision,digest,project_id FROM android_wire_sync_current WHERE owner_id=$1 AND entity_type=$2 AND entity_id=$3 AND team_id IS NULL AND tombstone=0").bind(s.owner.to_string()).bind(kind).bind(id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
                if item["revision"].as_i64()
                    != Some(row.try_get::<i64, _>("revision").map_err(storage_error)?)
                    || item["etag"].as_str()
                        != Some(
                            row.try_get::<String, _>("digest")
                                .map_err(storage_error)?
                                .as_str(),
                        )
                {
                    return Err(Error::Forbidden);
                }
                if let Some(project) = row
                    .try_get::<Option<String>, _>("project_id")
                    .map_err(storage_error)?
                {
                    let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_projects WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(project).bind(s.owner.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
                    if n != 1 {
                        return Err(Error::Forbidden);
                    }
                }
            }
        }
    }
    if s.target["kind"] == "personal_device" {
        let id = crate::android_personal::wire_uuid(&s.target, "device_id")?;
        let r=sqlx::query("SELECT registration,execution_revision FROM android_devices WHERE id=$1 AND identity_id=$2 AND team_id IS NULL AND revoked=0 AND draining=0").bind(id.to_string()).bind(s.owner.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        if Some(
            r.try_get::<i64, _>("execution_revision")
                .map_err(storage_error)?,
        ) != s.device_revision
            || Some(hash(
                r.try_get::<String, _>("registration")
                    .map_err(storage_error)?
                    .as_bytes(),
            )) != s.device_sha256
        {
            return Err(Error::Forbidden);
        }
    } else if s.target["kind"] != "server_linux" {
        return Err(Error::Forbidden);
    }
    Ok(())
}
impl Store {
    pub async fn android_schedule_original(
        &self,
        a: &Principal,
        key: &str,
        purpose: &str,
        request: &Value,
    ) -> Result<Option<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        self.identity()
            .require_global_session_in_transaction(&mut tx, a)
            .await
            .map_err(identity_error)?;
        self.identity()
            .require_android_management_in_transaction(&mut tx, a)
            .await
            .map_err(identity_error)?;
        if purpose == "occurrence_finalize" {
            let id = crate::android_personal::wire_uuid(request, "schedule_id")?;
            let s = record(&mut tx, a.identity_id(), id).await?;
            self.android_personal_permit(&mut tx, a, value(&s)?.project_id, ContentAction::Write)
                .await?;
        }
        let v = receipt(
            &mut tx,
            a.identity_id(),
            key,
            purpose,
            &digest(purpose, request),
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_schedule_receipt(
        &self,
        a: &Principal,
        key: &str,
        sha: &str,
    ) -> Result<Value> {
        operation(key)?;
        if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        self.identity()
            .require_global_session_in_transaction(&mut tx, a)
            .await
            .map_err(identity_error)?;
        self.identity()
            .require_android_management_in_transaction(&mut tx, a)
            .await
            .map_err(identity_error)?;
        let row=sqlx::query("SELECT purpose,request_sha256,receipt FROM android_private_schedule_operations WHERE owner_id=$1 AND operation_key=$2").bind(a.identity_id().to_string()).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        if row
            .try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != sha
        {
            return Err(Error::Conflict);
        }
        let purpose: String = row.try_get("purpose").map_err(storage_error)?;
        let original: Value = parse(&row.try_get::<String, _>("receipt").map_err(storage_error)?)?;
        if purpose == "occurrence_finalize" {
            let v: AndroidScheduleResolutionReceipt =
                serde_json::from_value(original.clone()).map_err(storage_error)?;
            let s = record(&mut tx, a.identity_id(), v.schedule_id).await?;
            if value(&s)?.project_id != v.schedule.project_id {
                return Err(Error::Forbidden);
            }
            self.android_personal_permit(&mut tx, a, v.schedule.project_id, ContentAction::Write)
                .await?;
        }
        let result = json!({"purpose":purpose,"request_sha256":sha,"receipt":original});
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_schedule_prepare(
        &self,
        a: &Principal,
        project: Uuid,
        thread: Option<Uuid>,
        task: Uuid,
        target: &Value,
        input: &Value,
        profile: Option<Uuid>,
        key: &str,
        runner: Option<&Value>,
        linux: bool,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<PreparedSchedule> {
        operation(key)?;
        if json_string(input)?.len() > 128 * 1024 || credential_fields(input) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let p = self
            .android_personal_permit(&mut tx, a, project, ContentAction::Execute)
            .await?;
        self.identity()
            .require_android_management_in_transaction(&mut tx, a)
            .await
            .map_err(identity_error)?;
        let identity = self
            .identity()
            .validate_principal_in_transaction(&mut tx, a)
            .await
            .map_err(identity_error)?;
        let session_revision:i64=sqlx::query_scalar("SELECT revision FROM identity_sessions WHERE id=$1 AND identity_id=$2 AND revoked=0 AND expires_at>$3").bind(a.session_id().to_string()).bind(a.identity_id().to_string()).bind(current).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let thread = match thread {
            Some(id) => {
                let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_threads WHERE id=$1 AND owner_id=$2 AND project_id=$3 AND deleted=0").bind(id.to_string()).bind(a.identity_id().to_string()).bind(project.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
                if n != 1 {
                    return Err(Error::Forbidden);
                }
                id
            }
            None => Uuid::new_v4(),
        };
        let raw:String=sqlx::query_scalar("SELECT payload FROM android_task_versions WHERE id=$1 AND project_id=$2 AND released=1 AND deleted=0 ORDER BY revision DESC LIMIT 1").bind(task.to_string()).bind(project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let task: Value = parse(&raw)?;
        let config = &task["config"];
        let task_runner = config
            .get("runner")
            .or_else(|| config.get("sync_metadata").and_then(|v| v.get("runner")));
        if task_runner.is_some_and(|v| !matches!(v.as_str(),Some("model"|"crew"|"workflow"))) {
            return Err(Error::Validation(
                "private_workflow_runner_unavailable".into(),
            ));
        }
        let mut frozen = match input {
            Value::Object(v) => v.clone(),
            Value::String(v) => serde_json::Map::from_iter([("prompt".into(), json!(v))]),
            Value::Null => serde_json::Map::new(),
            _ => return Err(device_invalid()),
        };
        for key in [
            "provider_binding",
            "frozen_runtime_context",
            "frozen_mcp_bindings",
            "frozen_github_binding",
            "frozen_private_runner",
            "frozen_project_files",
            "task_instructions",
            "task_config",
            "task_runner",
            "crew_definition",
            "crew_entity_id",
            "crew_revision",
            "schedule_origin",
            "private_runner_authority",
            "private_workflow",
            "device_crew",
        ] {
            frozen.remove(key);
        }
        let prompt = frozen
            .get("prompt")
            .or_else(|| frozen.get("text"))
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(task["instructions"].as_str().ok_or_else(device_invalid)?)
            .to_owned();
        if prompt.len() > 64000 {
            return Err(Error::BudgetExceeded);
        }
        frozen.insert("prompt".into(), json!(prompt));
        frozen.insert("task_instructions".into(), task["instructions"].clone());
        frozen.insert("task_config".into(), config.clone());
        frozen.insert("task_runner".into(), json!(task_runner.and_then(Value::as_str).unwrap_or("model")));
        if let Some(v) = frozen.get("output").filter(|v| !v.is_null()) {
            let output: StructuredOutput =
                serde_json::from_value(v.clone()).map_err(|_| device_invalid())?;
            cowork_domain::structured_output::validate_schema(&output)?;
        }
        let mut caps = task["required_capabilities"]
            .as_array()
            .filter(|v| v.len() <= 64)
            .ok_or_else(device_invalid)?
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(device_invalid))
            .collect::<Result<Vec<_>>>()?;
        let (device_revision, device_sha256) = match target["kind"].as_str() {
            Some("server_linux")
                if linux
                    && target.as_object().is_some_and(|o| o.len() <= 2)
                    && target.get("pool_id").is_none_or(Value::is_null) =>
            {
                let profile = profile.ok_or_else(device_invalid)?;
                let model =
                    crate::android_personal_provider::current(&mut tx, a.identity_id(), profile)
                        .await?;
                if model["has_secret"] != true && model["model_defaults"]["auth_mode"] != "none" {
                    return Err(Error::Forbidden);
                }
                frozen.insert(
                    "provider_binding".into(),
                    crate::android_personal_provider::binding(&model)?,
                );
                crate::actor_model_registry::check_reference(
                    &mut tx,
                    a.identity_id(),
                    project,
                    &model,
                    &frozen.get("model_binding").cloned().unwrap_or(Value::Null),
                )
                .await?;
                if let Some(runner) = runner {
                    frozen.insert("frozen_private_runner".into(), runner.clone());
                }
                if let Some(runner)=runner{frozen.insert("private_runner_authority".into(),crate::android_server_capabilities::runner_stamp(&mut tx,runner).await?);}
                let kind=task_runner.and_then(Value::as_str).unwrap_or("model");
                if kind!="model"{
                    let stamp=crate::android_private_workflows::engines::current(&mut tx,kind,if kind=="crew"{runner}else{None}).await?;
                    let workflow=crate::android_private_workflows::execution::freeze_in_tx(&mut tx,a.identity_id(),project,config,profile,keys,&stamp).await?.ok_or(Error::Forbidden)?;
                    frozen.insert("private_workflow".into(),workflow);
                    let capability=if kind=="crew"{"crew.python"}else{"workflow.pure"};if !caps.iter().any(|c|c==capability){caps.push(capability.into());}
                }
                (None, None)
            }
            Some("personal_device")
                if profile.is_none() && target.as_object().is_some_and(|o| o.len() == 2) =>
            {
                let kind=task_runner.and_then(Value::as_str).unwrap_or("model");
                if kind=="workflow"{return Err(Error::Validation("workflow_device_target_unavailable".into()))}
                if kind=="crew"{
                    let definition=crate::android_private_workflows::device_definition_in_tx(&mut tx,a.identity_id(),project,config,keys).await?;
                    frozen.insert("crew_definition".into(),definition);
                    if !caps.iter().any(|c|c=="crew.python"){caps.push("crew.python".into());}
                }
                let device = crate::android_personal::wire_uuid(target, "device_id")?;
                let row=sqlx::query("SELECT registration,execution_revision FROM android_devices WHERE id=$1 AND identity_id=$2 AND team_id IS NULL AND revoked=0 AND draining=0").bind(device.to_string()).bind(a.identity_id().to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
                let raw: String = row.try_get("registration").map_err(storage_error)?;
                let registration: DeviceExecutorRegistration = parse(&raw)?;
                if registration.kind != "personal_device"
                    || !caps
                        .iter()
                        .all(|v| registration.capabilities.iter().any(|c| &c.name == v))
                {
                    return Err(Error::Forbidden);
                }
                (
                    Some(row.try_get("execution_revision").map_err(storage_error)?),
                    Some(hash(raw.as_bytes())),
                )
            }
            _ => return Err(device_invalid()),
        };
        let factor=sqlx::query("SELECT m.factor_method,m.factor_revision,p.passkey_id FROM android_auth_session_metadata m LEFT JOIN android_auth_passkey_proofs p ON p.session_id=m.session_id AND p.owner_id=m.owner_id WHERE m.session_id=$1 AND m.owner_id=$2 AND m.revoked_at=0").bind(a.session_id().to_string()).bind(a.identity_id().to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let factor_pin = match factor {
            Some(r) => {
                json!({"method":r.try_get::<String,_>("factor_method").map_err(storage_error)?,"revision":r.try_get::<i64,_>("factor_revision").map_err(storage_error)?,"passkey_id":r.try_get::<Option<String>,_>("passkey_id").map_err(storage_error)?})
            }
            None => Value::Null,
        };
        let mut source = Source {
            owner: a.identity_id(),
            identity_revision: identity.revision,
            session: a.session_id(),
            session_revision,
            policy_revision: policy(&mut tx).await?,
            factor_pin,
            project,
            project_revision: p["revision"].as_i64().ok_or_else(device_invalid)?,
            thread,
            task: json!({"id":task["id"],"revision":task["revision"]}),
            target: target.clone(),
            input: Value::Object(frozen),
            profile,
            snapshot: None,
            capabilities: caps,
            runner: if target["kind"] == "server_linux" {
                runner.cloned()
            } else {
                None
            },
            device_revision,
            device_sha256,
        };
        tx.commit().await.map_err(storage_error)?;
        let mut metadata = serde_json::Map::new();
        for (key, kind) in [
            ("skill_ids", "skill"),
            ("memory_ids", "memory"),
            ("mcp_metadata_ids", "mcp_metadata"),
        ] {
            let selected = ids(&source.input, key)?;
            if !selected.is_empty() {
                metadata.insert(
                    kind.into(),
                    json!(
                        self.android_wire_runtime_metadata(a, project, kind, &selected)
                            .await?
                    ),
                );
            }
        }
        if !metadata.is_empty() {
            source.input["frozen_runtime_context"] = Value::Object(metadata);
        }
        if target["kind"] == "server_linux" {
            let selected = ids(&source.input, "mcp_metadata_ids")?;
            if !selected.is_empty() {
                source.input["frozen_mcp_bindings"] = json!(
                    self.android_private_mcp_freeze(a, project, &selected)
                        .await?
                );
            }
            if let Some(id) = source
                .input
                .get("github_binding_id")
                .filter(|v| !v.is_null())
            {
                let id = Uuid::parse_str(id.as_str().ok_or_else(device_invalid)?)
                    .map_err(|_| device_invalid())?;
                source.input["frozen_github_binding"] =
                    self.android_private_github_freeze(a, project, id).await?;
            }
        }
        let files = ids(&source.input, "project_file_ids")?;
        if !files.is_empty() {
            let (snapshot, bindings) = self
                .android_private_files_snapshot(
                    a,
                    project,
                    &files,
                    &format!("schedule-prepare:{key}"),
                    keys,
                    key_id,
                )
                .await?;
            source.snapshot = Some(snapshot);
            source.input["frozen_project_files"] = json!(bindings);
        }
        if json_string(&source)?.len() > 256 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        Ok(PreparedSchedule { source })
    }
    pub async fn android_schedules(
        &self,
        a: &Principal,
        project: Uuid,
    ) -> Result<Vec<AndroidScheduleRecord>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, a, project, ContentAction::Read)
            .await?;
        let rows=sqlx::query("SELECT * FROM android_private_schedules WHERE owner_id=$1 AND project_id=$2 AND deleted=0 ORDER BY id LIMIT 501").bind(a.identity_id().to_string()).bind(project.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 500 {
            return Err(Error::BudgetExceeded);
        }
        let v = rows.iter().map(value).collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_schedule_create_task(
        &self,
        a: &Principal,
        key: &str,
        body: &Value,
    ) -> Result<Value> {
        let purpose = "task_create";
        let sha = digest(purpose, body);
        let project = crate::android_personal::wire_uuid(body, "project_id")?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, a, project, ContentAction::Write)
            .await?;
        self.identity()
            .require_android_management_in_transaction(&mut tx, a)
            .await
            .map_err(identity_error)?;
        if let Some(v) = receipt(&mut tx, a.identity_id(), key, purpose, &sha).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let name = body["name"].as_str().ok_or_else(device_invalid)?;
        valid_text(name, 200)?;
        let instructions = body["instructions"].as_str().ok_or_else(device_invalid)?;
        valid_text(instructions, 64000)?;
        let caps = body["required_capabilities"]
            .as_array()
            .filter(|v| v.len() <= 64)
            .ok_or_else(device_invalid)?;
        if caps
            .iter()
            .any(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 100))
        {
            return Err(device_invalid());
        }
        let config = &body["config"];
        if !config.is_object()
            || json_string(config)?.len() > 128 * 1024
            || credential_fields(config)
            || config
                .get("runner")
                .is_some_and(|v| !matches!(v.as_str(),Some("model"|"crew"|"workflow")))
            || config
                .get("sync_metadata")
                .and_then(|v| v.get("runner"))
                .is_some_and(|v| !matches!(v.as_str(),Some("model"|"crew"|"workflow")))
        {
            return Err(Error::Validation(
                "private_workflow_runner_unavailable".into(),
            ));
        }
        let target = &body["default_target"];
        if !target.is_null()
            && !matches!(
                target["kind"].as_str(),
                Some("server_linux" | "personal_device")
            )
        {
            return Err(device_invalid());
        }
        let release = body["release"].as_bool().ok_or_else(device_invalid)?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(DISTINCT id) FROM android_task_versions WHERE project_id=$1 AND deleted=0").bind(project.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count >= 2000 {
            return Err(Error::BudgetExceeded);
        }
        let id = Uuid::new_v4();
        let v = json!({"schema_version":2,"id":id,"revision":1,"etag":format!("{id}:1"),"project_id":project,"name":name,"instructions":instructions,"required_capabilities":caps,"default_target":target,"config":config,"released":release,"created_at":device_iso(current)?,"deleted_at":null});
        sqlx::query("INSERT INTO android_task_versions(id,revision,project_id,payload,released,deleted) VALUES($1,1,$2,$3,$4,0)").bind(id.to_string()).bind(project.to_string()).bind(json_string(&v)?).bind(i64::from(release)).execute(&mut *tx).await.map_err(storage_error)?;
        crate::android_wire_sync::record(
            &mut tx,
            a.identity_id(),
            "task",
            id,
            None,
            Some(project),
            1,
            &v,
            false,
            current,
        )
        .await?;
        remember(&mut tx, a.identity_id(), key, purpose, &sha, &v, current).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_schedule(&self, a: &Principal, id: Uuid) -> Result<AndroidScheduleRecord> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let row = record(&mut tx, a.identity_id(), id).await?;
        let v = value(&row)?;
        self.android_personal_permit(&mut tx, a, v.project_id, ContentAction::Read)
            .await?;
        if v.deleted_at.is_some() {
            return Err(Error::NotFound);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    /// request is the exact canonical wire body (update includes its path UUID).
    pub async fn android_schedule_write(
        &self,
        a: &Principal,
        id: Option<Uuid>,
        key: &str,
        request: &Value,
        prepared: Option<PreparedSchedule>,
        new_thread: Option<&str>,
    ) -> Result<Value> {
        let purpose = if id.is_some() {
            "update"
        } else if new_thread.is_some() {
            "thread_create"
        } else {
            "create"
        };
        let sha = digest(purpose, request);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        self.identity()
            .require_global_session_in_transaction(&mut tx, a)
            .await
            .map_err(identity_error)?;
        self.identity()
            .require_android_management_in_transaction(&mut tx, a)
            .await
            .map_err(identity_error)?;
        if let Some(v) = receipt(&mut tx, a.identity_id(), key, purpose, &sha).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let old = if let Some(id) = id {
            Some(record(&mut tx, a.identity_id(), id).await?)
        } else {
            None
        };
        let old_value = old.as_ref().map(value).transpose()?;
        let body = if id.is_some() {
            &request["body"]
        } else {
            request
        };
        let enabled = body["enabled"].as_bool().ok_or_else(device_invalid)?;
        if enabled && let Some(id) = id {
            let retired: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM android_private_schedule_resolutions WHERE schedule_id=$1 AND owner_id=$2 AND action='retire'")
                .bind(id.to_string()).bind(a.identity_id().to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if retired != 0 {
                return Err(Error::Conflict);
            }
        }
        let project = if let Some(v) = &old_value {
            v.project_id
        } else {
            crate::android_personal::wire_uuid(body, "project_id")?
        };
        self.android_personal_permit(
            &mut tx,
            a,
            project,
            if enabled {
                ContentAction::Execute
            } else {
                ContentAction::Write
            },
        )
        .await?;
        let task = if let Some(v) = &old_value {
            v.task_id
        } else {
            crate::android_personal::wire_uuid(body, "task_id")?
        };
        let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_task_versions WHERE id=$1 AND project_id=$2 AND deleted=0").bind(task.to_string()).bind(project.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if n == 0 && (old.is_none() || enabled) {
            return Err(Error::NotFound);
        }
        if let Some(v) = &old_value {
            if v.deleted_at.is_some() || body["expected_revision"].as_i64() != Some(v.revision) {
                return Err(Error::Conflict);
            }
            if enabled
                && old
                    .as_ref()
                    .ok_or(Error::Conflict)?
                    .try_get::<Option<String>, _>("active_occurrence_id")
                    .map_err(storage_error)?
                    .is_some()
            {
                return Err(Error::Conflict);
            }
        }
        let cron = crate::android_schedule_cron::normalize(
            body["cron"].as_str().ok_or_else(device_invalid)?,
            body["timezone"].as_str().ok_or_else(device_invalid)?,
        )?;
        let timezone = body["timezone"]
            .as_str()
            .ok_or_else(device_invalid)?
            .to_owned();
        let input = body["input"].clone();
        if json_string(&input)?.len() > 128 * 1024 || credential_fields(&input) {
            return Err(device_invalid());
        }
        let profile = body["model_profile_id"]
            .as_str()
            .map(|s| Uuid::parse_str(s).map_err(|_| device_invalid()))
            .transpose()?;
        if let Some(profile) = profile {
            if old_value
                .as_ref()
                .is_none_or(|v| v.model_profile_id != Some(profile))
            {
                crate::android_personal_provider::current(&mut tx, a.identity_id(), profile)
                    .await?;
            }
        }
        let target = body["executor_target"].clone();
        if !matches!(
            target["kind"].as_str(),
            Some("server_linux" | "personal_device")
        ) {
            return Err(device_invalid());
        }
        match target["kind"].as_str() {
            Some("server_linux")
                if target.as_object().is_some_and(|v| v.len() <= 2)
                    && target.get("pool_id").is_none_or(Value::is_null) => {}
            Some("personal_device")
                if target.as_object().is_some_and(|v| v.len() == 2) && profile.is_none() =>
            {
                let device = crate::android_personal::wire_uuid(&target, "device_id")?;
                if old_value
                    .as_ref()
                    .is_none_or(|v| v.executor_target != target)
                {
                    let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_devices WHERE id=$1 AND identity_id=$2 AND team_id IS NULL AND revoked=0").bind(device.to_string()).bind(a.identity_id().to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
                    if n != 1 {
                        return Err(Error::Forbidden);
                    }
                }
            }
            _ => return Err(device_invalid()),
        }
        let source = prepared.map(|v| v.source);
        if enabled && source.is_none() {
            return Err(Error::Forbidden);
        }
        let thread = if let Some(v) = &old_value {
            v.thread_id
        } else if new_thread.is_some() {
            source
                .as_ref()
                .map(|s| s.thread)
                .unwrap_or_else(Uuid::new_v4)
        } else {
            crate::android_personal::wire_uuid(body, "thread_id")?
        };
        let new_thread_value = if let Some(title) = new_thread {
            let title = title.trim();
            valid_text(title, 200)?;
            let v = json!({"schema_version":2,"id":thread,"revision":1,"etag":format!("{thread}:1"),"project_id":project,"forked_from_thread_id":null,"forked_from_message_id":null,"title":title,"deleted_at":null,"created_at":device_iso(current)?});
            sqlx::query("INSERT INTO android_personal_threads(id,project_id,owner_id,revision,payload,deleted,creation_key,request_sha256) VALUES($1,$2,$3,1,$4,0,$5,$6)").bind(thread.to_string()).bind(project.to_string()).bind(a.identity_id().to_string()).bind(json_string(&v)?).bind(format!("schedule-thread:{}",hash(key.as_bytes()))).bind(&sha).execute(&mut *tx).await.map_err(storage_error)?;
            crate::android_wire_sync::record(
                &mut tx,
                a.identity_id(),
                "thread",
                thread,
                None,
                Some(project),
                1,
                &v,
                false,
                current,
            )
            .await?;
            Some(v)
        } else {
            let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_threads WHERE id=$1 AND project_id=$2 AND owner_id=$3 AND deleted=0").bind(thread.to_string()).bind(project.to_string()).bind(a.identity_id().to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if n != 1 {
                return Err(Error::Forbidden);
            }
            None
        };
        if let Some(s) = &source {
            if s.owner != a.identity_id()
                || s.session != a.session_id()
                || s.project != project
                || s.thread != thread
                || s.target != target
                || s.profile != profile
                || crate::android_personal::wire_uuid(&s.task, "id")? != task
            {
                return Err(Error::Forbidden);
            }
            check_source(&mut tx, s, current).await?;
        }
        let id = old_value
            .as_ref()
            .map(|v| v.id)
            .unwrap_or_else(Uuid::new_v4);
        let revision = old_value
            .as_ref()
            .map_or(Ok(1), |v| v.revision.checked_add(1).ok_or(Error::Conflict))?;
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
        let due = if enabled {
            crate::android_schedule_cron::next(&cron, &timezone, current)?
        } else {
            None
        };
        let enabled = enabled && due.is_some();
        let mut v = AndroidScheduleRecord {
            schema_version: 2,
            id,
            revision,
            etag: format!("{id}:{revision}"),
            task_id: task,
            project_id: project,
            thread_id: thread,
            cron,
            timezone,
            executor_target: target,
            input,
            model_profile_id: profile,
            enabled,
            next_run_at: due.map(device_iso).transpose()?,
            last_triggered_at: old_value.as_ref().and_then(|v| v.last_triggered_at.clone()),
            blocked_reason: None,
            created_at: old_value
                .as_ref()
                .map(|v| v.created_at.clone())
                .unwrap_or(device_iso(current)?),
            updated_at: device_iso(current)?,
            deleted_at: None,
        };
        if old.is_none() {
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_schedules WHERE owner_id=$1 AND project_id=$2 AND deleted=0").bind(a.identity_id().to_string()).bind(project.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if count >= 500 {
                return Err(Error::BudgetExceeded);
            }
            sqlx::query("INSERT INTO android_private_schedules(id,owner_id,project_id,thread_id,revision,execution_revision,enabled,deleted,payload) VALUES($1,$2,$3,$4,$5,$6,$7,0,$8)").bind(id.to_string()).bind(a.identity_id().to_string()).bind(project.to_string()).bind(thread.to_string()).bind(revision).bind(execution).bind(i64::from(enabled)).bind(json_string(&v)?).execute(&mut *tx).await.map_err(storage_error)?;
        }
        sqlx::query("UPDATE android_private_schedule_grants SET active=0 WHERE schedule_id=$1 AND owner_id=$2").bind(id.to_string()).bind(a.identity_id().to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        let grant = if enabled {
            let source = source.ok_or(Error::Forbidden)?;
            let grant = Uuid::new_v4();
            let raw = json_string(&source)?;
            sqlx::query("INSERT INTO android_private_schedule_grants(id,schedule_id,owner_id,execution_revision,active,source,source_sha256,created_at) VALUES($1,$2,$3,$4,1,$5,$6,$7)").bind(grant.to_string()).bind(id.to_string()).bind(a.identity_id().to_string()).bind(execution).bind(&raw).bind(hash(raw.as_bytes())).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
            Some(grant.to_string())
        } else {
            None
        };
        if body["enabled"] == true && !enabled {
            v.blocked_reason = Some("cron_has_no_future_occurrence".into());
        }
        sqlx::query("UPDATE android_private_schedules SET execution_revision=$1,grant_id=$2 WHERE id=$3 AND owner_id=$4").bind(execution).bind(grant).bind(id.to_string()).bind(a.identity_id().to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        saved(&mut tx, &v).await?;
        crate::android_wire_sync::record(
            &mut tx,
            a.identity_id(),
            "schedule",
            id,
            None,
            Some(project),
            revision,
            &serde_json::to_value(&v).map_err(storage_error)?,
            false,
            current,
        )
        .await?;
        let result = if let Some(thread) = new_thread_value {
            json!({"schema_version":2,"thread":thread,"schedule":v})
        } else {
            serde_json::to_value(&v).map_err(storage_error)?
        };
        remember(
            &mut tx,
            a.identity_id(),
            key,
            purpose,
            &sha,
            &result,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_schedule_delete(
        &self,
        a: &Principal,
        id: Uuid,
        key: &str,
        expected: Option<i64>,
    ) -> Result<()> {
        let request = json!({"id":id,"expected_revision":expected});
        let sha = digest("delete", &request);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        self.identity()
            .require_global_session_in_transaction(&mut tx, a)
            .await
            .map_err(identity_error)?;
        self.identity()
            .require_android_management_in_transaction(&mut tx, a)
            .await
            .map_err(identity_error)?;
        if receipt(&mut tx, a.identity_id(), key, "delete", &sha)
            .await?
            .is_some()
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(());
        }
        let row = record(&mut tx, a.identity_id(), id).await?;
        let mut v = value(&row)?;
        self.android_personal_permit(&mut tx, a, v.project_id, ContentAction::Write)
            .await?;
        if expected.is_some_and(|r| r != v.revision) {
            return Err(Error::Conflict);
        }
        if v.deleted_at.is_none() {
            v.revision = v.revision.checked_add(1).ok_or(Error::Conflict)?;
            v.etag = format!("{id}:{}", v.revision);
            v.deleted_at = Some(device_iso(current)?);
            v.updated_at = device_iso(current)?;
            v.enabled = false;
            v.next_run_at = None;
            saved(&mut tx, &v).await?;
            sqlx::query("UPDATE android_private_schedule_grants SET active=0 WHERE schedule_id=$1 AND owner_id=$2").bind(id.to_string()).bind(a.identity_id().to_string()).execute(&mut *tx).await.map_err(storage_error)?;
            crate::android_wire_sync::record(
                &mut tx,
                a.identity_id(),
                "schedule",
                id,
                None,
                Some(v.project_id),
                v.revision,
                &serde_json::to_value(&v).map_err(storage_error)?,
                true,
                current,
            )
            .await?;
        }
        remember(
            &mut tx,
            a.identity_id(),
            key,
            "delete",
            &sha,
            &json!({"deleted":id,"revision":v.revision}),
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_schedule_resolution(
        &self,
        a: &Principal,
        schedule: Uuid,
        occurrence: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<AndroidScheduleResolutionView> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        self.identity()
            .require_android_management_in_transaction(&mut tx, a)
            .await
            .map_err(identity_error)?;
        let s = record(&mut tx, a.identity_id(), schedule).await?;
        self.android_personal_permit(&mut tx, a, value(&s)?.project_id, ContentAction::Read)
            .await?;
        let proof = resolution_evidence(
            self,
            &mut tx,
            &s,
            a.identity_id(),
            occurrence,
            current,
            keys,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(proof.view)
    }
    /// Finalizes this occurrence only. No response creates a replacement run or execution grant.
    pub async fn android_schedule_finalize(
        &self,
        a: &Principal,
        schedule: Uuid,
        occurrence: Uuid,
        key: &str,
        request: &AndroidScheduleResolutionRequest,
        keys: &dyn KeyStore,
    ) -> Result<AndroidScheduleResolutionReceipt> {
        operation(key)?;
        if request.expected_revision <= 0
            || !valid_sha(&request.evidence_sha256)
            || !matches!(request.action.as_str(), "resolve" | "retire")
        {
            return Err(device_invalid());
        }
        let original = json!({"schedule_id":schedule,"occurrence_id":occurrence,"body":request});
        let sha = digest("occurrence_finalize", &original);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        self.identity()
            .require_android_management_in_transaction(&mut tx, a)
            .await
            .map_err(identity_error)?;
        let s = record(&mut tx, a.identity_id(), schedule).await?;
        let mut v = value(&s)?;
        self.android_personal_permit(&mut tx, a, v.project_id, ContentAction::Write)
            .await?;
        // Fresh rights precede every immutable replay; old grants never authorize a resolution.
        if let Some(old) =
            receipt(&mut tx, a.identity_id(), key, "occurrence_finalize", &sha).await?
        {
            let result: AndroidScheduleResolutionReceipt =
                serde_json::from_value(old).map_err(storage_error)?;
            if result.schedule_id != schedule || result.occurrence_id != occurrence {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(result);
        }
        if v.revision != request.expected_revision {
            return Err(Error::Conflict);
        }
        let proof = resolution_evidence(
            self,
            &mut tx,
            &s,
            a.identity_id(),
            occurrence,
            current,
            keys,
        )
        .await?;
        if proof.view.evidence_sha256 != request.evidence_sha256
            || proof.view.original_resolution.is_some()
            || if request.action == "resolve" {
                !proof.view.can_resolve
            } else {
                !proof.view.can_retire
            }
        {
            return Err(Error::Conflict);
        }
        // This positive absence proof was observed under the same writer fence as device/server leasing.
        if proof.view.never_dispatched && proof.view.run_state == "queued" {
            if proof.view.family == "server_linux" {
                crate::android_private_server::save(
                    &mut tx,
                    &proof.run,
                    "canceled",
                    None,
                    Some(&json!({"code":"schedule_never_dispatched","retryable":false})),
                    current,
                )
                .await?;
            } else {
                let mut payload: Value = parse(
                    &proof
                        .run
                        .try_get::<String, _>("payload")
                        .map_err(storage_error)?,
                )?;
                let rev = proof
                    .run
                    .try_get::<i64, _>("revision")
                    .map_err(storage_error)?
                    .checked_add(1)
                    .ok_or(Error::Conflict)?;
                payload["state"] = json!("canceled");
                payload["revision"] = json!(rev);
                payload["etag"] = json!(format!("{}:{rev}", proof.view.run_id));
                payload["updated_at"] = json!(device_iso(current)?);
                payload["finished_at"] = json!(device_iso(current)?);
                payload["error"] = json!({"code":"schedule_never_dispatched","retryable":false});
                crate::android_device_runs::save_run(
                    &mut tx, &proof.run, &payload, "canceled", current,
                )
                .await?;
                crate::android_device_runs::device_event(
                    &mut tx,
                    proof.view.run_id,
                    None,
                    "run_canceled",
                    &json!({"never_dispatched":true,"occurrence_id":occurrence}),
                    current,
                )
                .await?;
            }
        }
        let retired = request.action == "retire";
        let outcome = if proof.view.never_dispatched {
            "never_dispatched"
        } else if retired && proof.view.unresolved_effects {
            "retired_unknown_effect"
        } else if proof.view.run_state == "completed" {
            "resolved_completed"
        } else {
            "resolved_stopped"
        };
        // Unknown evidence remains unknown forever; retirement only closes the scheduling marker.
        let occurrence_status = if retired && proof.view.unresolved_effects {
            "unknown"
        } else if proof.view.never_dispatched {
            "canceled"
        } else if proof.view.run_state == "completed" {
            "completed"
        } else if proof.view.run_state == "canceled" {
            "canceled"
        } else {
            "failed"
        };
        one(sqlx::query("UPDATE android_private_schedule_occurrences SET status=$1,error=$2,completed_at=COALESCE(completed_at,$3) WHERE id=$4 AND schedule_id=$5 AND owner_id=$6 AND run_id=$7 AND creation_key=$8 AND request_sha256=$9 AND spec_sha256=$10 AND status=$11")
            .bind(occurrence_status).bind(if retired && proof.view.unresolved_effects {Some("schedule_retired_unknown_effect")} else {None::<&str>})
            .bind(current).bind(occurrence.to_string()).bind(schedule.to_string()).bind(a.identity_id().to_string())
            .bind(proof.view.run_id.to_string()).bind(&proof.view.original_creation_key).bind(&proof.view.original_request_sha256)
            .bind(&proof.view.original_spec_sha256).bind(&proof.view.occurrence_status).execute(&mut *tx).await.map_err(storage_error)?.rows_affected())?;
        one(sqlx::query("UPDATE android_private_schedules SET active_occurrence_id=NULL,execution_revision=execution_revision+1,grant_id=NULL WHERE id=$1 AND owner_id=$2 AND active_occurrence_id=$3 AND revision=$4")
            .bind(schedule.to_string()).bind(a.identity_id().to_string()).bind(occurrence.to_string()).bind(v.revision)
            .execute(&mut *tx).await.map_err(storage_error)?.rows_affected())?;
        sqlx::query("UPDATE android_private_schedule_grants SET active=0 WHERE schedule_id=$1 AND owner_id=$2")
            .bind(schedule.to_string()).bind(a.identity_id().to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        v.enabled = false;
        v.next_run_at = None;
        v.blocked_reason = Some(
            if retired {
                "schedule_permanently_retired"
            } else {
                "schedule_resolved_reauthorization_required"
            }
            .into(),
        );
        v.revision = v.revision.checked_add(1).ok_or(Error::Conflict)?;
        v.etag = format!("{}:{}", v.id, v.revision);
        v.updated_at = device_iso(current)?;
        saved(&mut tx, &v).await?;
        publish(&mut tx, a.identity_id(), &v, current).await?;
        let result = AndroidScheduleResolutionReceipt {
            schema_version: 2,
            id: Uuid::new_v4(),
            schedule_id: schedule,
            occurrence_id: occurrence,
            run_id: proof.view.run_id,
            grant_id: proof.view.grant_id,
            original_creation_key: proof.view.original_creation_key,
            original_request_sha256: proof.view.original_request_sha256,
            original_spec_sha256: proof.view.original_spec_sha256,
            evidence_sha256: proof.view.evidence_sha256,
            dispatch_sha256: proof.view.dispatch_sha256,
            terminal_receipt_sha256: proof.view.terminal_receipt_sha256,
            action: request.action.clone(),
            outcome: outcome.into(),
            unresolved_effects: proof.view.unresolved_effects,
            schedule_retired: retired,
            finalized_at: device_iso(current)?,
            schedule: v,
        };
        let encoded = json_string(&result)?;
        sqlx::query("INSERT INTO android_private_schedule_resolutions(id,owner_id,schedule_id,occurrence_id,operation_key,request_sha256,action,outcome,evidence_sha256,evidence,receipt,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
            .bind(result.id.to_string()).bind(a.identity_id().to_string()).bind(schedule.to_string()).bind(occurrence.to_string())
            .bind(key).bind(&sha).bind(&result.action).bind(outcome).bind(&result.evidence_sha256).bind(json_string(&proof.evidence)?)
            .bind(encoded).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        remember(
            &mut tx,
            a.identity_id(),
            key,
            "occurrence_finalize",
            &sha,
            &serde_json::to_value(&result).map_err(storage_error)?,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_schedule_history(
        &self,
        a: &Principal,
        id: Uuid,
        after: i64,
        limit: i64,
    ) -> Result<AndroidScheduleHistory> {
        if after < 0 || !(1..=200).contains(&limit) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let row = record(&mut tx, a.identity_id(), id).await?;
        let v = value(&row)?;
        self.android_personal_permit(&mut tx, a, v.project_id, ContentAction::Read)
            .await?;
        let rows=sqlx::query("SELECT * FROM android_private_schedule_occurrences WHERE schedule_id=$1 AND owner_id=$2 AND sequence>$3 ORDER BY sequence LIMIT $4").bind(id.to_string()).bind(a.identity_id().to_string()).bind(after).bind(limit+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let more = rows.len() > limit as usize;
        let mut out = Vec::new();
        for row in rows.iter().take(limit as usize) {
            let request: ScheduledRun =
                parse(&row.try_get::<String, _>("request").map_err(storage_error)?)?;
            out.push(AndroidScheduleOccurrence {
                id: uuid(row, "id")?,
                schedule_id: id,
                run_id: uuid(row, "run_id")?,
                sequence: row.try_get("sequence").map_err(storage_error)?,
                executor_target: request.target,
                due_at: device_iso(row.try_get("due_at").map_err(storage_error)?)?,
                triggered_at: device_iso(row.try_get("triggered_at").map_err(storage_error)?)?,
                status: row.try_get("status").map_err(storage_error)?,
                error: row.try_get("error").map_err(storage_error)?,
                completed_at: row
                    .try_get::<Option<i64>, _>("completed_at")
                    .map_err(storage_error)?
                    .map(device_iso)
                    .transpose()?,
                missed_occurrences: u32::try_from(
                    row.try_get::<i64, _>("missed_occurrences")
                        .map_err(storage_error)?,
                )
                .map_err(storage_error)?,
                catch_up_truncated: row
                    .try_get::<i64, _>("catch_up_truncated")
                    .map_err(storage_error)?
                    == 1,
            });
        }
        let cursor = if more {
            out.last().map(|v| v.sequence)
        } else {
            None
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(AndroidScheduleHistory {
            occurrences: out,
            next_cursor: cursor,
        })
    }
    /// SQL admission only. The existing private/device workers own every external effect.
    pub async fn android_schedule_tick<F: Fn() -> bool + Send + Sync>(
        &self,
        linux: bool,
        runner: Option<&Value>,
        admit: F,
    ) -> Result<usize> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        reconcile(&mut tx, current).await?;
        let rows=sqlx::query("SELECT * FROM android_private_schedules WHERE enabled=1 AND deleted=0 AND active_occurrence_id IS NULL AND next_run<=$1 ORDER BY next_run,id LIMIT 16").bind(current).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let mut admitted = 0;
        for r in rows {
            if !admit() {
                return Err(Error::Forbidden);
            }
            let mut v = value(&r)?;
            // A Windows API node leaves Linux grants for the actual Linux worker.
            if v.executor_target["kind"] == "server_linux" && !linux {
                continue;
            }
            let source = match grant_source(&mut tx, &r, current).await {
                Ok(s) => s,
                Err(_) => {
                    block(&mut tx, &mut v, "schedule_authority_changed", current).await?;
                    continue;
                }
            };
            if source.target["kind"] == "server_linux" && source.runner.as_ref() != runner {
                block(&mut tx, &mut v, "schedule_runner_changed", current).await?;
                continue;
            }
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM android_private_schedule_occurrences WHERE schedule_id=$1",
            )
            .bind(v.id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
            if count >= 10000 {
                block(&mut tx, &mut v, "schedule_history_limit", current).await?;
                continue;
            }
            let due = r
                .try_get::<Option<i64>, _>("next_run")
                .map_err(storage_error)?
                .ok_or(Error::Conflict)?;
            let (missed, truncated) =
                crate::android_schedule_cron::missed(&v.cron, &v.timezone, due, current)?;
            let sequence: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(sequence),0)+1 FROM android_private_schedule_occurrences",
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
            let occurrence = Uuid::new_v4();
            let run = Uuid::new_v4();
            let key = format!("{KEY_PREFIX}{}:{occurrence}", v.id);
            let mut input = source.input.clone();
            if source.target["kind"] == "server_linux" {
                crate::android_server_capabilities::validate_runner(&mut tx,&input).await?;
            }
            input["schedule_origin"] = json!({"schedule_id":v.id,"occurrence_id":occurrence,"due_at":device_iso(due)?,"triggered_at":device_iso(current)?,"missed_occurrences":missed,"catch_up_truncated":truncated});
            let request = ScheduledRun {
                owner: source.owner,
                session: source.session,
                session_revision: source.session_revision,
                project: source.project,
                project_revision: source.project_revision,
                thread: source.thread,
                profile: source.profile,
                snapshot: source.snapshot,
                target: source.target.clone(),
                input,
                task: source.task.clone(),
                capabilities: source.capabilities.clone(),
                run,
                key,
            };
            let raw = json_string(&request)?;
            let sha = hash(raw.as_bytes());
            let family = source.target["kind"].as_str().ok_or_else(device_invalid)?;
            sqlx::query("INSERT INTO android_private_schedule_occurrences(id,schedule_id,owner_id,grant_id,execution_revision,sequence,run_id,creation_key,family,request_sha256,spec_sha256,request,status,due_at,triggered_at,missed_occurrences,catch_up_truncated) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'',$11,'queued',$12,$13,$14,$15)").bind(occurrence.to_string()).bind(v.id.to_string()).bind(source.owner.to_string()).bind(r.try_get::<String,_>("grant_id").map_err(storage_error)?).bind(r.try_get::<i64,_>("execution_revision").map_err(storage_error)?).bind(sequence).bind(run.to_string()).bind(&request.key).bind(family).bind(&sha).bind(raw).bind(due).bind(current).bind(i64::from(missed)).bind(i64::from(truncated)).execute(&mut *tx).await.map_err(storage_error)?;
            one(sqlx::query("UPDATE android_private_schedules SET active_occurrence_id=$1 WHERE id=$2 AND owner_id=$3 AND active_occurrence_id IS NULL AND revision=$4").bind(occurrence.to_string()).bind(v.id.to_string()).bind(source.owner.to_string()).bind(v.revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected())?;
            if !admit() {
                return Err(Error::Forbidden);
            }
            let result = if family == "server_linux" {
                self.android_private_server_create_scheduled(&mut tx, &request, current)
                    .await?
            } else {
                self.android_device_create_scheduled(&mut tx, &request, current)
                    .await?
            };
            one(sqlx::query("UPDATE android_private_schedule_occurrences SET spec_sha256=$1 WHERE id=$2 AND run_id=$3 AND spec_sha256=''").bind(canonical_hash(&result["spec"])).bind(occurrence.to_string()).bind(run.to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected())?;
            v.last_triggered_at = Some(device_iso(current)?);
            v.updated_at = device_iso(current)?;
            v.revision = v.revision.checked_add(1).ok_or(Error::Conflict)?;
            v.etag = format!("{}:{}", v.id, v.revision);
            saved(&mut tx, &v).await?;
            publish(&mut tx, source.owner, &v, current).await?;
            admitted += 1;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(admitted)
    }
}
async fn grant_source(
    tx: &mut Transaction<'_, Any>,
    schedule: &AnyRow,
    current: i64,
) -> Result<Source> {
    let id = uuid(schedule, "id")?;
    let owner = uuid(schedule, "owner_id")?;
    let execution: i64 = schedule
        .try_get("execution_revision")
        .map_err(storage_error)?;
    if schedule
        .try_get::<i64, _>("enabled")
        .map_err(storage_error)?
        != 1
        || schedule
            .try_get::<i64, _>("deleted")
            .map_err(storage_error)?
            != 0
    {
        return Err(Error::Forbidden);
    }
    let grant=sqlx::query("SELECT source,source_sha256 FROM android_private_schedule_grants WHERE id=$1 AND schedule_id=$2 AND owner_id=$3 AND execution_revision=$4 AND active=1").bind(schedule.try_get::<String,_>("grant_id").map_err(storage_error)?).bind(id.to_string()).bind(owner.to_string()).bind(execution).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let raw: String = grant.try_get("source").map_err(storage_error)?;
    if hash(raw.as_bytes())
        != grant
            .try_get::<String, _>("source_sha256")
            .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let source: Source = parse(&raw)?;
    if source.owner != owner
        || source.project != uuid(schedule, "project_id")?
        || source.thread != uuid(schedule, "thread_id")?
    {
        return Err(Error::Forbidden);
    }
    check_source(tx, &source, current).await?;
    Ok(source)
}
pub(crate) async fn validate_prepared(
    tx: &mut Transaction<'_, Any>,
    q: &ScheduledRun,
    current: i64,
) -> Result<()> {
    let r=sqlx::query("SELECT * FROM android_private_schedule_occurrences WHERE run_id=$1 AND creation_key=$2 AND owner_id=$3 AND status='queued'").bind(q.run.to_string()).bind(&q.key).bind(q.owner.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    if !q.key.starts_with(KEY_PREFIX)
        || r.try_get::<String, _>("request").map_err(storage_error)? != json_string(q)?
        || r.try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != hash(json_string(q)?.as_bytes())
        || r.try_get::<String, _>("family").map_err(storage_error)?
            != q.target["kind"].as_str().ok_or_else(device_invalid)?
    {
        return Err(Error::Forbidden);
    }
    let s = record(tx, q.owner, uuid(&r, "schedule_id")?).await?;
    let source = grant_source(tx, &s, current).await?;
    if uuid(&r, "grant_id")? != uuid(&s, "grant_id")?
        || r.try_get::<i64, _>("execution_revision")
            .map_err(storage_error)?
            != s.try_get::<i64, _>("execution_revision")
                .map_err(storage_error)?
        || s.try_get::<Option<String>, _>("active_occurrence_id")
            .map_err(storage_error)?
            .as_deref()
            != Some(
                r.try_get::<String, _>("id")
                    .map_err(storage_error)?
                    .as_str(),
            )
        || source.session != q.session
        || source.session_revision != q.session_revision
        || source.project != q.project
        || source.project_revision != q.project_revision
        || source.thread != q.thread
        || source.profile != q.profile
        || source.snapshot != q.snapshot
        || source.target != q.target
        || source.task != q.task
        || source.capabilities != q.capabilities
    {
        return Err(Error::Forbidden);
    }
    let mut original = q.input.clone();
    if q.target["kind"] == "server_linux" {
        crate::android_server_capabilities::validate_runner(tx, &q.input).await?;
        original
            .as_object_mut()
            .ok_or_else(device_invalid)?
            .remove("private_runner_authority");
    }
    original
        .as_object_mut()
        .ok_or_else(device_invalid)?
        .remove("schedule_origin");
    if original != source.input {
        return Err(Error::Forbidden);
    }
    Ok(())
}
/// True only for an actual stored original scheduled run. Missing/fake origins fail closed.
pub(crate) async fn validate_origin(
    tx: &mut Transaction<'_, Any>,
    run: &AnyRow,
    family: &str,
    current: i64,
) -> Result<bool> {
    validate_origin_mode(tx, run, family, current, false).await
}
pub(crate) async fn validate_original_receipt(
    tx: &mut Transaction<'_, Any>,
    run: &AnyRow,
    family: &str,
    current: i64,
) -> Result<bool> {
    validate_origin_mode(tx, run, family, current, true).await
}
async fn validate_origin_mode(
    tx: &mut Transaction<'_, Any>,
    run: &AnyRow,
    family: &str,
    current: i64,
    receipt_only: bool,
) -> Result<bool> {
    let key: String = run.try_get("creation_key").map_err(storage_error)?;
    let id = uuid(run, "id")?;
    let origin = sqlx::query("SELECT * FROM android_private_schedule_occurrences WHERE run_id=$1")
        .bind(id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?;
    let Some(origin) = origin else {
        return if key.starts_with(KEY_PREFIX) {
            Err(Error::Forbidden)
        } else {
            Ok(false)
        };
    };
    let q: ScheduledRun = parse(
        &origin
            .try_get::<String, _>("request")
            .map_err(storage_error)?,
    )?;
    let owner = if family == "server_linux" {
        uuid(run, "owner_id")?
    } else {
        if run
            .try_get::<Option<String>, _>("team_id")
            .map_err(storage_error)?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
        uuid(run, "creator_id")?
    };
    let session = if family == "server_linux" {
        uuid(run, "session_id")?
    } else {
        uuid(run, "creator_session_id")?
    };
    let session_revision: i64 = run
        .try_get(if family == "server_linux" {
            "session_revision"
        } else {
            "creator_session_revision"
        })
        .map_err(storage_error)?;
    let payload: Value = parse(&run.try_get::<String, _>("payload").map_err(storage_error)?)?;
    let sha: String = run
        .try_get(if family == "server_linux" {
            "request_sha"
        } else {
            "request_sha256"
        })
        .map_err(storage_error)?;
    if origin
        .try_get::<String, _>("family")
        .map_err(storage_error)?
        != family
        || q.run != id
        || q.owner != owner
        || q.session != session
        || q.session_revision != session_revision
        || q.key != key
        || sha
            != origin
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
        || canonical_hash(&payload["spec"])
            != origin
                .try_get::<String, _>("spec_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    if receipt_only {
        if !matches!(
            run.try_get::<String, _>("state")
                .map_err(storage_error)?
                .as_str(),
            "completed" | "failed" | "canceled"
        ) {
            return Err(Error::Forbidden);
        }
        let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM identity_subjects i JOIN android_personal_projects p ON p.owner_id=i.id WHERE i.id=$1 AND i.active=1 AND p.id=$2 AND p.deleted=0").bind(owner.to_string()).bind(q.project.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
        if n != 1 {
            return Err(Error::Forbidden);
        }
        return Ok(true);
    }
    if family == "server_linux" {
        crate::android_server_capabilities::validate_runner(tx, &payload["spec"]["input"]).await?;
    }
    if !matches!(
        origin
            .try_get::<String, _>("status")
            .map_err(storage_error)?
            .as_str(),
        "queued" | "running"
    ) {
        return Err(Error::Forbidden);
    }
    let s = record(tx, owner, uuid(&origin, "schedule_id")?).await?;
    let source = grant_source(tx, &s, current).await?;
    if uuid(&origin, "grant_id")? != uuid(&s, "grant_id")?
        || origin
            .try_get::<i64, _>("execution_revision")
            .map_err(storage_error)?
            != s.try_get::<i64, _>("execution_revision")
                .map_err(storage_error)?
        || s.try_get::<Option<String>, _>("active_occurrence_id")
            .map_err(storage_error)?
            .as_deref()
            != Some(
                origin
                    .try_get::<String, _>("id")
                    .map_err(storage_error)?
                    .as_str(),
            )
        || source.project != uuid(run, "project_id")?
        || source.project_revision
            != run
                .try_get::<i64, _>("project_revision")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(true)
}
async fn publish(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    v: &AndroidScheduleRecord,
    current: i64,
) -> Result<()> {
    crate::android_wire_sync::record(
        tx,
        owner,
        "schedule",
        v.id,
        None,
        Some(v.project_id),
        v.revision,
        &serde_json::to_value(v).map_err(storage_error)?,
        v.deleted_at.is_some(),
        current,
    )
    .await?;
    Ok(())
}
fn valid_sha(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
struct ResolutionEvidence {
    view: AndroidScheduleResolutionView,
    evidence: Value,
    run: AnyRow,
}
/// Observes original rows under device_writer. It cannot accept a caller's receipt or physical witness.
async fn resolution_evidence(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    s: &AnyRow,
    owner: Uuid,
    occurrence: Uuid,
    current: i64,
    keys: &dyn KeyStore,
) -> Result<ResolutionEvidence> {
    let schedule = value(s)?;
    let o = sqlx::query("SELECT * FROM android_private_schedule_occurrences WHERE id=$1 AND schedule_id=$2 AND owner_id=$3")
        .bind(occurrence.to_string()).bind(schedule.id.to_string()).bind(owner.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let raw: String = o.try_get("request").map_err(storage_error)?;
    let q: ScheduledRun = parse(&raw)?;
    let family: String = o.try_get("family").map_err(storage_error)?;
    let original_sha: String = o.try_get("request_sha256").map_err(storage_error)?;
    let spec_sha: String = o.try_get("spec_sha256").map_err(storage_error)?;
    let grant = uuid(&o, "grant_id")?;
    let g = sqlx::query("SELECT * FROM android_private_schedule_grants WHERE id=$1 AND owner_id=$2 AND schedule_id=$3")
        .bind(grant.to_string()).bind(owner.to_string()).bind(schedule.id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let source_raw: String = g.try_get("source").map_err(storage_error)?;
    let source: Source = parse(&source_raw)?;
    if q.run != uuid(&o, "run_id")?
        || q.owner != owner
        || q.project != schedule.project_id
        || q.thread != schedule.thread_id
        || q.key
            != o.try_get::<String, _>("creation_key")
                .map_err(storage_error)?
        || q.key != format!("{KEY_PREFIX}{}:{occurrence}", schedule.id)
        || hash(raw.as_bytes()) != original_sha
        || !valid_sha(&spec_sha)
        || hash(source_raw.as_bytes())
            != g.try_get::<String, _>("source_sha256")
                .map_err(storage_error)?
        || source.owner != owner
        || source.project != q.project
        || source.thread != q.thread
        || source.target != q.target
        || source.session != q.session
        || source.session_revision != q.session_revision
        || source.profile != q.profile
        || source.task != q.task
        || source.snapshot != q.snapshot
        || source.project_revision != q.project_revision
        || source.capabilities != q.capabilities
        || o.try_get::<i64, _>("execution_revision")
            .map_err(storage_error)?
            != g.try_get::<i64, _>("execution_revision")
                .map_err(storage_error)?
        || q.target["kind"].as_str() != Some(family.as_str())
    {
        return Err(Error::Forbidden);
    }
    let mut original_input = q.input.clone();
    original_input
        .as_object_mut()
        .ok_or(Error::Forbidden)?
        .remove("schedule_origin");
    if family == "server_linux" {
        original_input
            .as_object_mut()
            .ok_or(Error::Forbidden)?
            .remove("private_runner_authority");
    }
    if original_input != source.input {
        return Err(Error::Forbidden);
    }
    let server = family == "server_linux";
    if !server && family != "personal_device" {
        return Err(Error::Forbidden);
    }
    let table = if server {
        "android_private_server_runs"
    } else {
        "android_scoped_runs"
    };
    let run = sqlx::query(&format!("SELECT * FROM {table} WHERE id=$1"))
        .bind(q.run.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::Conflict)?;
    let payload: Value = parse(&run.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if uuid(&run, if server { "owner_id" } else { "creator_id" })? != owner
        || uuid(&run, "project_id")? != q.project
        || uuid(&run, if server { "thread_id" } else { "order_id" })? != q.thread
        || uuid(
            &run,
            if server {
                "session_id"
            } else {
                "creator_session_id"
            },
        )? != q.session
        || run
            .try_get::<i64, _>(if server {
                "session_revision"
            } else {
                "creator_session_revision"
            })
            .map_err(storage_error)?
            != q.session_revision
        || run
            .try_get::<String, _>("creation_key")
            .map_err(storage_error)?
            != q.key
        || run
            .try_get::<String, _>(if server {
                "request_sha"
            } else {
                "request_sha256"
            })
            .map_err(storage_error)?
            != original_sha
        || canonical_hash(&payload["spec"]) != spec_sha
        || payload["spec"]["id"] != q.run.to_string()
        || run
            .try_get::<i64, _>("project_revision")
            .map_err(storage_error)?
            != q.project_revision
        || (server && uuid(&run, "provider_id")? != q.profile.ok_or(Error::Forbidden)?)
        || (!server
            && run
                .try_get::<Option<String>, _>("team_id")
                .map_err(storage_error)?
                .is_some())
    {
        return Err(Error::Forbidden);
    }
    let state: String = run.try_get("state").map_err(storage_error)?;
    if payload["state"] != state {
        return Err(Error::Forbidden);
    }
    let status: String = o.try_get("status").map_err(storage_error)?;
    let retired: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM android_private_schedule_resolutions WHERE schedule_id=$1 AND owner_id=$2 AND action='retire'")
        .bind(schedule.id.to_string()).bind(owner.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    let old = sqlx::query("SELECT receipt,evidence,evidence_sha256 FROM android_private_schedule_resolutions WHERE occurrence_id=$1 AND owner_id=$2 AND schedule_id=$3")
        .bind(occurrence.to_string()).bind(owner.to_string()).bind(schedule.id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let mut view = AndroidScheduleResolutionView {
        schema_version: 2,
        schedule_id: schedule.id,
        occurrence_id: occurrence,
        run_id: q.run,
        grant_id: grant,
        original_creation_key: q.key.clone(),
        original_request_sha256: original_sha.clone(),
        original_spec_sha256: spec_sha.clone(),
        family: family.clone(),
        occurrence_status: status.clone(),
        run_state: state.clone(),
        schedule_revision: schedule.revision,
        evidence_sha256: String::new(),
        dispatch_sha256: String::new(),
        terminal_receipt_sha256: None,
        physical_stop_verified: false,
        physical_quiescence_verified: false,
        never_created_guest: false,
        never_dispatched: false,
        unresolved_effects: true,
        can_resolve: false,
        can_retire: false,
        block_reason: None,
        schedule_retired: retired != 0,
        original_resolution: None,
    };
    if let Some(old) = old {
        let original: AndroidScheduleResolutionReceipt =
            parse(&old.try_get::<String, _>("receipt").map_err(storage_error)?)?;
        let proof: Value = parse(
            &old.try_get::<String, _>("evidence")
                .map_err(storage_error)?,
        )?;
        if original.occurrence_id != occurrence
            || original.run_id != q.run
            || original.grant_id != grant
            || original.original_creation_key != q.key
            || original.original_request_sha256 != original_sha
            || original.original_spec_sha256 != spec_sha
            || canonical_hash(&proof) != original.evidence_sha256
            || old
                .try_get::<String, _>("evidence_sha256")
                .map_err(storage_error)?
                != original.evidence_sha256
        {
            return Err(Error::Forbidden);
        }
        view.evidence_sha256 = original.evidence_sha256.clone();
        view.dispatch_sha256 = original.dispatch_sha256.clone();
        view.terminal_receipt_sha256 = original.terminal_receipt_sha256.clone();
        view.unresolved_effects = original.unresolved_effects;
        view.never_dispatched = original.outcome == "never_dispatched";
        view.physical_stop_verified = proof["physical_stop_verified"] == true;
        view.physical_quiescence_verified = proof["physical_quiescence_verified"] == true;
        view.never_created_guest = proof["never_created_guest"] == true;
        view.block_reason = Some("schedule_occurrence_already_finalized".into());
        view.original_resolution = Some(original);
        return Ok(ResolutionEvidence {
            view,
            evidence: proof,
            run,
        });
    }
    let mut dispatch = Vec::new();
    let mut physical = Vec::new();
    let mut unresolved = false;
    let mut physical_safe = true;
    let physical_stop: bool;
    let mut no_guest = false;
    let lease_until: i64;
    let never: bool;
    let terminal_sha: Option<String>;
    if server {
        lease_until = run.try_get("lease_until").map_err(storage_error)?;
        let intent: Option<String> = run.try_get("effect_intent").map_err(storage_error)?;
        let received: Option<String> = run.try_get("effect_received").map_err(storage_error)?;
        if let Some(intent) = &intent {
            let intent_value: Value = parse(intent)?;
            let evidence = received.as_ref().map(|v| parse::<Value>(v)).transpose()?;
            let confirmed = evidence.as_ref().is_some_and(|v| {
                v["uncertain"] != true && v["wire_evidence"]["completed_stream"] == true
            });
            unresolved |= !confirmed;
            dispatch.push(json!({"kind":"legacy_model","intent_sha256":canonical_hash(&intent_value),"receipt_sha256":evidence.as_ref().map(canonical_hash),"confirmed":confirmed}));
        } else if received.is_some() {
            return Err(Error::Forbidden);
        }
        let mut unconfirmed_steps = Vec::new();
        let steps = sqlx::query("SELECT * FROM android_private_loop_steps WHERE run_id=$1 ORDER BY sequence LIMIT 10001")
            .bind(q.run.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if steps.len() > 10000 {
            return Err(Error::BudgetExceeded);
        }
        for step in &steps {
            let n: i64 = step.try_get("sequence").map_err(storage_error)?;
            let step_intent: Value =
                serde_json::from_slice(&crate::android_private_workspace::open_value(
                    keys,
                    &run,
                    &format!("intent:{n}"),
                    &parse(&step.try_get::<String, _>("intent").map_err(storage_error)?)?,
                )?)
                .map_err(storage_error)?;
            let intent_sha: String = step.try_get("request_sha").map_err(storage_error)?;
            if canonical_hash(&step_intent) != intent_sha {
                return Err(Error::Forbidden);
            }
            let step_receipt: Option<String> = step.try_get("receipt").map_err(storage_error)?;
            let receipt_sha: Option<String> = step.try_get("receipt_sha").map_err(storage_error)?;
            let kind: String = step.try_get("kind").map_err(storage_error)?;
            let step_state: String = step.try_get("state").map_err(storage_error)?;
            let confirmed = if let Some(sealed) = step_receipt {
                let received: Value =
                    serde_json::from_slice(&crate::android_private_workspace::open_value(
                        keys,
                        &run,
                        &format!("receipt:{n}"),
                        &parse(&sealed)?,
                    )?)
                    .map_err(storage_error)?;
                if receipt_sha.as_deref() != Some(canonical_hash(&received).as_str()) {
                    return Err(Error::Forbidden);
                }
                let binding = if kind == "model" {
                    received["intent_sha256"] == intent_sha
                } else if kind == "tool" {
                    received["request_sha256"] == intent_sha
                } else {
                    // Process/MCP formats also bind at the original protected receipt writer.
                    // AEAD AAD authenticates this exact run+sequence; never accept a conflicting explicit binding.
                    ["intent_sha256", "request_sha256", "request_sha"]
                        .iter()
                        .all(|k| received.get(*k).is_none_or(|v| v == &json!(intent_sha)))
                };
                step_state == "received"
                    && binding
                    && received["uncertain"] != true
                    && (kind != "model" || received["wire_evidence"]["completed_stream"] == true)
            } else {
                if receipt_sha.is_some() {
                    return Err(Error::Forbidden);
                }
                false
            };
            if !confirmed {
                unconfirmed_steps.push((
                    step.try_get::<String, _>("invocation_id")
                        .map_err(storage_error)?,
                    intent_sha.clone(),
                    kind.clone(),
                ));
            }
            dispatch.push(json!({"sequence":n,"invocation_id":step.try_get::<String,_>("invocation_id").map_err(storage_error)?,"kind":kind,
                "lease_sha256":hash(step.try_get::<String,_>("lease_id").map_err(storage_error)?.as_bytes()),"checkpoint_sha256":step.try_get::<String,_>("checkpoint_sha").map_err(storage_error)?,
                "intent_sha256":intent_sha,"receipt_sha256":receipt_sha,"state":step_state,"confirmed":confirmed}));
        }
        let containers = sqlx::query(
            "SELECT * FROM android_private_containers WHERE run_id=$1 ORDER BY id LIMIT 10001",
        )
        .bind(q.run.to_string())
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
        if containers.len() > 10000 {
            return Err(Error::BudgetExceeded);
        }
        let mut actual_stops = 0;
        let mut no_create_steps = std::collections::BTreeSet::new();
        for c in &containers {
            let nonce = uuid(c, "id")?;
            let binding: Value = parse(
                &c.try_get::<String, _>("original_binding")
                    .map_err(storage_error)?,
            )?;
            if uuid(c, "owner_id")? != owner
                || uuid(c, "project_id")? != q.project
                || binding != payload["spec"]["input"]["frozen_private_runner"]
            {
                return Err(Error::Forbidden);
            }
            let witness: Option<Value> = c
                .try_get::<Option<String>, _>("termination_witness")
                .map_err(storage_error)?
                .map(|v| parse(&v))
                .transpose()?;
            let container_id: Option<String> = c.try_get("container_id").map_err(storage_error)?;
            let c_state: String = c.try_get("state").map_err(storage_error)?;
            let removed = c_state == "removed"
                && witness.as_ref().is_some_and(|w| {
                    w["engine_id"] == binding["engine_id"]
                        && w["nonce"] == nonce.to_string()
                        && w["removed"] == true
                        && w["running"] == false
                        && w["paused"] == false
                        && w["restarting"] == false
                        && w["pid"] == 0
                        && w["original_id"] == json!(container_id)
                        && w["stop"].is_object()
                });
            let no_create = if c
                .try_get::<Option<String>, _>("original_unsent_receipt")
                .map_err(storage_error)?
                .is_some()
            {
                store
                    .android_private_container_no_create_receipt(tx, nonce)
                    .await?
            } else {
                None
            };
            if let Some(proof) = &no_create {
                no_create_steps.insert((
                    c.try_get::<String, _>("invocation_id")
                        .map_err(storage_error)?,
                    proof["original"]["original_request_sha256"]
                        .as_str()
                        .ok_or(Error::Forbidden)?
                        .to_owned(),
                ));
            }
            physical_safe &= removed || no_create.is_some();
            actual_stops += usize::from(removed);
            no_guest |= no_create.is_some();
            physical.push(json!({"nonce":nonce,"binding_sha256":canonical_hash(&binding),"creation_sha256":c.try_get::<String,_>("creation_sha").map_err(storage_error)?,
                "lease_sha256":hash(c.try_get::<String,_>("lease_id").map_err(storage_error)?.as_bytes()),"revision":c.try_get::<i64,_>("revision").map_err(storage_error)?,"state":c_state,
                "witness_sha256":witness.as_ref().map(canonical_hash),"removed":removed,"no_create_receipt_sha256":no_create.as_ref().map(canonical_hash),"physical_evidence":removed}));
        }
        physical_stop = !containers.is_empty() && actual_stops == containers.len();
        unresolved |= unconfirmed_steps.iter().any(|(invocation, sha, kind)| {
            kind != "process" || !no_create_steps.contains(&(invocation.clone(), sha.clone()))
        });
        let checkpoint: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_private_loop_checkpoints WHERE run_id=$1",
        )
        .bind(q.run.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
        let (sessions, sessions_safe) =
            resolution_sessions(store, tx, &run, q.run, keys, &mut physical, &mut unresolved)
                .await?;
        physical_safe &= sessions_safe;
        let lease: Option<String> = run.try_get("lease_id").map_err(storage_error)?;
        never = lease.is_none()
            && lease_until == 0
            && intent.is_none()
            && received.is_none()
            && steps.is_empty()
            && containers.is_empty()
            && checkpoint == 0
            && sessions == 0
            && payload["started_at"].is_null();
        let result: Option<Value> = sqlx::query_scalar::<_, String>(
            "SELECT payload FROM android_private_loop_results WHERE run_id=$1",
        )
        .bind(q.run.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .map(|v| parse(&v))
        .transpose()?;
        if let Some(result) = &result {
            if payload["result"] != *result {
                return Err(Error::Forbidden);
            }
        }
        terminal_sha = if never {
            None
        } else if !unresolved {
            Some(canonical_hash(
                &json!({"state":state,"payload_sha256":canonical_hash(&payload),"dispatch":dispatch,"result_sha256":result.as_ref().map(canonical_hash)}),
            ))
        } else {
            None
        };
    } else {
        let executor = uuid(&run, "executor_id")?;
        if q.target["device_id"] != executor.to_string()
            || source.device_revision
                != Some(
                    run.try_get::<i64, _>("executor_revision")
                        .map_err(storage_error)?,
                )
        {
            return Err(Error::Forbidden);
        }
        let lease: Option<String> = run.try_get("lease_token").map_err(storage_error)?;
        let credential: Option<String> = run.try_get("credential_id").map_err(storage_error)?;
        let credential_revision: Option<i64> =
            run.try_get("credential_revision").map_err(storage_error)?;
        let expiry: Option<i64> = run.try_get("lease_expires_at").map_err(storage_error)?;
        let completion: Option<String> = run.try_get("completion_sha256").map_err(storage_error)?;
        lease_until = expiry.unwrap_or(0);
        let rows=sqlx::query("SELECT sequence,request_sha256,payload FROM android_scoped_events WHERE run_id=$1 ORDER BY sequence LIMIT 100001")
            .bind(q.run.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if rows.len() > 100000 {
            return Err(Error::BudgetExceeded);
        }
        let mut leased_event = false;
        let mut terminal_event = false;
        for e in rows {
            let event: Value = parse(&e.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let kind = event["kind"].as_str().ok_or(Error::Forbidden)?;
            if event["run_id"] != q.run.to_string()
                || hash(json_string(&(kind, &event["payload"]))?.as_bytes())
                    != e.try_get::<String, _>("request_sha256")
                        .map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
            leased_event |= kind == "run_leased";
            terminal_event |= (state == "completed" && kind == "run_completed")
                || (state == "failed" && kind == "run_failed");
            dispatch.push(json!({"sequence":e.try_get::<i64,_>("sequence").map_err(storage_error)?,"kind":kind,"event_sha256":canonical_hash(&event)}));
        }
        never = lease.is_none()
            && credential.is_none()
            && credential_revision.is_none()
            && expiry.is_none()
            && completion.is_none()
            && !leased_event
            && payload["started_at"].is_null();
        let completed = matches!(state.as_str(), "completed" | "failed")
            && completion.as_ref().is_some_and(|v| valid_sha(v))
            && lease.is_some()
            && credential.is_some()
            && credential_revision.is_some()
            && leased_event
            && terminal_event;
        physical_safe = never || completed;
        physical_stop = completed;
        unresolved = !physical_safe;
        dispatch.push(json!({"executor_id":executor,"executor_revision":run.try_get::<i64,_>("executor_revision").map_err(storage_error)?,
            "lease_sha256":lease.as_ref().map(|v|hash(v.as_bytes())),"credential_id":credential,"credential_revision":credential_revision,"lease_until":expiry,"completion_sha256":completion,"terminal_event":terminal_event}));
        terminal_sha = if completed {
            Some(canonical_hash(
                &json!({"completion_sha256":completion,"payload_sha256":canonical_hash(&payload),"dispatch":dispatch}),
            ))
        } else {
            None
        };
    }
    let terminal = matches!(
        state.as_str(),
        "completed" | "failed" | "canceled" | "interrupted" | "unknown" | "expired"
    );
    let live = lease_until > current;
    let active = s
        .try_get::<Option<String>, _>("active_occurrence_id")
        .map_err(storage_error)?
        .as_deref()
        == Some(occurrence.to_string().as_str());
    // Even a pristine queued occurrence requires explicit fresh actor authorization to cancel it.
    let eligible = active && (terminal || (never && state == "queued")) && physical_safe && !live;
    let block = if !active {
        Some("schedule_occurrence_not_active")
    } else if live {
        Some("schedule_original_lease_active")
    } else if !terminal && !(never && state == "queued") {
        Some("schedule_original_run_active")
    } else if !physical_safe {
        Some("schedule_physical_stop_unverified")
    } else if unresolved {
        Some("schedule_unknown_effect_retirement_only")
    } else {
        None
    };
    let evidence = json!({"domain":"actor-schedule-resolution/v1","owner_id":owner,"schedule_id":schedule.id,"schedule_revision":schedule.revision,
        "occurrence_id":occurrence,"occurrence_status":status,"run_id":q.run,"grant_id":grant,"creation_key":q.key,"request_sha256":original_sha,"spec_sha256":spec_sha,
        "grant_source_sha256":hash(source_raw.as_bytes()),"family":family,"run_state":state,"run_payload_sha256":canonical_hash(&payload),"dispatch":dispatch,"physical":physical,
        "active_marker":active,"lease_live":live,"never_dispatched":never,"physical_stop_verified":physical_stop,"physical_quiescence_verified":physical_safe,"never_created_guest":no_guest,"unresolved_effects":unresolved,"terminal_receipt_sha256":terminal_sha});
    if json_string(&evidence)?.len() > 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    view.evidence_sha256 = canonical_hash(&evidence);
    view.dispatch_sha256 = canonical_hash(&json!(dispatch));
    view.terminal_receipt_sha256 = terminal_sha;
    view.physical_stop_verified = physical_stop;
    view.physical_quiescence_verified = physical_safe;
    view.never_created_guest = no_guest;
    view.never_dispatched = never;
    view.unresolved_effects = unresolved;
    view.can_resolve = eligible && !unresolved;
    view.can_retire = eligible;
    view.block_reason = block.map(str::to_owned);
    Ok(ResolutionEvidence {
        view,
        evidence,
        run,
    })
}
async fn resolution_sessions(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    run: &AnyRow,
    id: Uuid,
    keys: &dyn KeyStore,
    evidence: &mut Vec<Value>,
    unresolved: &mut bool,
) -> Result<(usize, bool)> {
    let mut total = 0;
    let mut physical = true;
    for (table, kind) in [
        ("android_private_browser_sessions", "browser"),
        ("android_private_terminal_sessions", "terminal"),
    ] {
        let sessions = sqlx::query(&format!(
            "SELECT * FROM {table} WHERE run_id=$1 ORDER BY id LIMIT 1001"
        ))
        .bind(id.to_string())
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
        if sessions.len() > 1000 {
            return Err(Error::BudgetExceeded);
        }
        total += sessions.len();
        for s in sessions {
            let sid = uuid(&s, "id")?;
            if uuid(&s, "owner_id")? != uuid(run, "owner_id")?
                || uuid(&s, "project_id")? != uuid(run, "project_id")?
            {
                return Err(Error::Forbidden);
            }
            let lease: String = s.try_get("lease_id").map_err(storage_error)?;
            let nonce: Option<String> = s.try_get("container_nonce").map_err(storage_error)?;
            let removed: i64 = if let Some(nonce) = &nonce {
                sqlx::query_scalar("SELECT COUNT(*) FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND state='removed' AND termination_witness IS NOT NULL")
                .bind(nonce).bind(id.to_string()).bind(&lease).fetch_one(&mut **tx).await.map_err(storage_error)?
            } else {
                0
            };
            // The container's actual witness is independently verified by resolution_evidence.
            let state: String = s.try_get("state").map_err(storage_error)?;
            let unsent = if let Some(nonce) = &nonce {
                let original:Option<String> = sqlx::query_scalar("SELECT original_unsent_receipt FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3")
                    .bind(nonce).bind(id.to_string()).bind(&lease).fetch_optional(&mut **tx).await.map_err(storage_error)?.flatten();
                if original.is_some() {
                    store
                        .android_private_container_no_create_receipt(
                            tx,
                            Uuid::parse_str(nonce).map_err(storage_error)?,
                        )
                        .await?
                } else {
                    None
                }
            } else {
                None
            };
            let no_start = kind == "terminal"
                && unsent.as_ref().is_some_and(|v| {
                    v["terminal_no_start"]["session_id"] == sid.to_string()
                        && v["terminal_no_start"]["guest_started"] == false
                        && v["terminal_no_start"]["physical_evidence"] == false
                })
                && state == "closed"
                && [
                    "input_sequence",
                    "output_sequence",
                    "input_bytes",
                    "output_bytes",
                ]
                .iter()
                .all(|k| s.try_get::<i64, _>(*k).ok() == Some(0));
            physical &= removed == 1 || (kind == "browser" && unsent.is_some()) || no_start;
            let mut requests = Vec::new();
            let (request_table, order) = if kind == "browser" {
                ("android_private_browser_requests", "request_id")
            } else {
                ("android_private_terminal_inputs", "sequence")
            };
            let rows = sqlx::query(&format!(
                "SELECT * FROM {request_table} WHERE session_id=$1 ORDER BY {order} LIMIT 10001"
            ))
            .bind(sid.to_string())
            .fetch_all(&mut **tx)
            .await
            .map_err(storage_error)?;
            if rows.len() > 10000 {
                return Err(Error::BudgetExceeded);
            }
            *unresolved |= state != "closed" && !(unsent.is_some() && rows.is_empty());
            for request in rows {
                let intent_id: String = request.try_get("intent_version").map_err(storage_error)?;
                let intent =
                    crate::android_private_workspace::version_bytes(tx, run, &intent_id, keys)
                        .await?;
                let sha: String = request.try_get("request_sha").map_err(storage_error)?;
                if hash(&intent) != sha {
                    return Err(Error::Forbidden);
                }
                let receipt_id: Option<String> =
                    request.try_get("receipt_version").map_err(storage_error)?;
                let request_state: String = request.try_get("state").map_err(storage_error)?;
                let observed = if let Some(receipt_id) = &receipt_id {
                    let bytes =
                        crate::android_private_workspace::version_bytes(tx, run, receipt_id, keys)
                            .await?;
                    let received: Value = serde_json::from_slice(&bytes).map_err(storage_error)?;
                    if kind == "browser"
                        && (received["run"] != id.to_string()
                            || received["session"] != sid.to_string()
                            || received["lease"] != lease
                            || received["nonce"] != json!(nonce)
                            || received["original_request_sha256"] != sha)
                    {
                        return Err(Error::Forbidden);
                    }
                    Some(hash(&bytes))
                } else {
                    None
                };
                *unresolved |= request_state != "received" || observed.is_none();
                requests.push(json!({"intent_version":intent_id,"request_sha256":sha,"receipt_version":receipt_id,"receipt_sha256":observed,"state":request_state}));
            }
            let closure_sha = if kind == "terminal" {
                let receipt: Option<String> =
                    s.try_get("receipt_version").map_err(storage_error)?;
                if let Some(receipt) = receipt {
                    let bytes =
                        crate::android_private_workspace::version_bytes(tx, run, &receipt, keys)
                            .await?;
                    let closed: Value = serde_json::from_slice(&bytes).map_err(storage_error)?;
                    if closed["session_id"] != sid.to_string()
                        || closed["container_nonce"] != json!(nonce)
                        || closed["original_physical_removed"] != true
                    {
                        return Err(Error::Forbidden);
                    }
                    *unresolved |=
                        closed["input_uncertain"] != false || closed["original_closed"].is_null();
                    Some(hash(&bytes))
                } else if no_start {
                    Some(canonical_hash(
                        &unsent.as_ref().ok_or(Error::Forbidden)?["terminal_no_start"],
                    ))
                } else {
                    *unresolved = true;
                    None
                }
            } else {
                None
            };
            evidence.push(json!({"kind":kind,"session_id":sid,"revision":s.try_get::<i64,_>("revision").map_err(storage_error)?,
                "state":state,"lease_sha256":hash(lease.as_bytes()),"container_nonce":nonce,"removed":removed==1,"no_create_sha256":unsent.as_ref().map(canonical_hash),"never_started":no_start,"closure_sha256":closure_sha,"requests":requests}));
        }
    }
    Ok((total, physical))
}
async fn block(
    tx: &mut Transaction<'_, Any>,
    v: &mut AndroidScheduleRecord,
    reason: &str,
    current: i64,
) -> Result<()> {
    v.enabled = false;
    v.next_run_at = None;
    v.blocked_reason = Some(reason.into());
    v.updated_at = device_iso(current)?;
    v.revision = v.revision.checked_add(1).ok_or(Error::Conflict)?;
    v.etag = format!("{}:{}", v.id, v.revision);
    saved(tx, v).await?;
    sqlx::query("UPDATE android_private_schedule_grants SET active=0 WHERE schedule_id=$1")
        .bind(v.id.to_string())
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    let owner: String =
        sqlx::query_scalar("SELECT owner_id FROM android_private_schedules WHERE id=$1")
            .bind(v.id.to_string())
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    publish(
        tx,
        Uuid::parse_str(&owner).map_err(storage_error)?,
        v,
        current,
    )
    .await?;
    Ok(())
}
async fn reconcile(tx: &mut Transaction<'_, Any>, current: i64) -> Result<()> {
    let rows=sqlx::query("SELECT * FROM android_private_schedule_occurrences WHERE status IN('queued','running') ORDER BY sequence LIMIT 64").fetch_all(&mut **tx).await.map_err(storage_error)?;
    for o in rows {
        let run = uuid(&o, "run_id")?;
        let family: String = o.try_get("family").map_err(storage_error)?;
        let table = if family == "server_linux" {
            "android_private_server_runs"
        } else {
            "android_scoped_runs"
        };
        let row = sqlx::query(&format!("SELECT * FROM {table} WHERE id=$1"))
            .bind(run.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
        let Some(row) = row else {
            return Err(Error::Conflict);
        };
        let state: String = row.try_get("state").map_err(storage_error)?;
        if !matches!(
            state.as_str(),
            "completed" | "failed" | "canceled" | "interrupted" | "unknown" | "expired"
        ) {
            if state == "running" {
                sqlx::query("UPDATE android_private_schedule_occurrences SET status='running' WHERE id=$1 AND status='queued'").bind(uuid(&o,"id")?.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
            }
            continue;
        }
        let mut unknown = matches!(state.as_str(), "interrupted" | "unknown");
        if family == "server_linux" {
            let pending:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM android_private_containers WHERE run_id=$1 AND state<>'removed' AND original_unsent_receipt IS NULL)+(SELECT COUNT(*) FROM android_private_loop_steps WHERE run_id=$1 AND state<>'received')").bind(run.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
            unknown |= pending != 0;
        }
        let status = if unknown {
            "unknown"
        } else if state == "expired" {
            "failed"
        } else {
            state.as_str()
        };
        let error = if unknown {
            Some("schedule_original_effect_unresolved")
        } else if state == "completed" {
            None
        } else {
            Some("schedule_run_stopped")
        };
        sqlx::query("UPDATE android_private_schedule_occurrences SET status=$1,error=$2,completed_at=$3 WHERE id=$4 AND status IN('queued','running')").bind(status).bind(error).bind(current).bind(uuid(&o,"id")?.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
        let s = record(tx, uuid(&o, "owner_id")?, uuid(&o, "schedule_id")?).await?;
        let mut v = value(&s)?;
        if unknown {
            block(tx, &mut v, "schedule_original_effect_unresolved", current).await?;
            continue;
        }
        one(sqlx::query("UPDATE android_private_schedules SET active_occurrence_id=NULL WHERE id=$1 AND active_occurrence_id=$2").bind(v.id.to_string()).bind(uuid(&o,"id")?.to_string()).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
        v.next_run_at = if v.enabled && v.deleted_at.is_none() {
            crate::android_schedule_cron::next(&v.cron, &v.timezone, current)?
                .map(device_iso)
                .transpose()?
        } else {
            None
        };
        if v.enabled && v.next_run_at.is_none() {
            v.enabled = false;
            v.blocked_reason = Some("cron_has_no_future_occurrence".into());
        }
        v.updated_at = device_iso(current)?;
        v.revision = v.revision.checked_add(1).ok_or(Error::Conflict)?;
        v.etag = format!("{}:{}", v.id, v.revision);
        saved(tx, &v).await?;
        publish(tx, uuid(&o, "owner_id")?, &v, current).await?;
    }
    Ok(())
}
