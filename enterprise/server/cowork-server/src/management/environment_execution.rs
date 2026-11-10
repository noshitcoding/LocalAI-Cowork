//! At-most-once process tasks on an original qualified environment runtime.
use super::*;
use axum::extract::Query;
#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    format: u32,
    request_id: Uuid,
    workflow_id: Uuid,
    step_id: Uuid,
    organisation_id: Uuid,
    instance_id: Uuid,
    execution_epoch: Uuid,
    user_id: Uuid,
    management_session_id: Uuid,
    environment_id: Uuid,
    expected_environment_revision: i64,
    profile_sha256: String,
    task_input: Value,
    capabilities: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Read {
    organisation_id: Uuid,
    instance_id: Uuid,
    user_id: Uuid,
    management_session_id: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Operator {
    executor_host_binding: executor_hosts::HostBinding,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Completion {
    executor_host_binding: executor_hosts::HostBinding,
    lease_token: Uuid,
    plan_sha256: String,
    result: Value,
}
fn conflict() -> ApiError {
    ApiError::Conflict("original environment execution authority or runtime changed".into())
}
fn hash(v: &impl serde::Serialize) -> Result<String, ApiError> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(v)?)))
}
pub(super) async fn unlocked(
    tx: &mut Transaction<'_, Postgres>,
    instance: Uuid,
) -> Result<(), ApiError> {
    let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_environment_executions WHERE instance_id=$1 AND state IN ('queued','running','held'))").bind(instance).fetch_one(&mut **tx).await?;
    if busy {
        return Err(conflict());
    }
    Ok(())
}
async fn authority(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    plan: &Value,
) -> Result<(), ApiError> {
    let org: Uuid = serde_json::from_value(plan["organisation_id"].clone())?;
    let instance: Uuid = serde_json::from_value(plan["instance_id"].clone())?;
    let user: Uuid = serde_json::from_value(plan["user_id"].clone())?;
    environment_access::require(
        tx,
        org,
        instance,
        serde_json::from_value(plan["environment"]["id"].clone())?,
        user,
        environment_access::Role::Runner,
    )
    .await?;
    if !crate::fleet::allowed_on(
        &mut **tx,
        &crate::fleet::AccessRequest {
            organisation_id: org,
            instance_id: instance,
            user_id: user,
        },
    )
    .await?
    {
        return Err(denied());
    }
    job_authority::validate(state, tx, &plan["actor_authority"], org).await?;
    if !plan["environment_controller"].is_null()
        && vm_controllers::current(
            tx,
            org,
            instance,
            plan["template"]["backend"].as_str().ok_or_else(conflict)?,
        )
        .await?
        .as_ref()
            != Some(&plan["environment_controller"])
    {
        return Err(conflict());
    }
    let current:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 AND organisation_id=$2 FOR SHARE").bind(instance).bind(org).fetch_one(&mut **tx).await?;
    let env:Value=sqlx::query_scalar("SELECT to_jsonb(e) FROM management_environments e WHERE id=$1 AND instance_id=$2 AND organisation_id=$3 FOR SHARE").bind(serde_json::from_value::<Uuid>(plan["environment"]["id"].clone())?).bind(instance).bind(org).fetch_one(&mut **tx).await?;
    if current["state"] != "ready"
        || current["revision"] != plan["instance"]["revision"]
        || current["execution_epoch"] != plan["instance"]["execution_epoch"]
        || env != plan["environment"]
        || env["state"] != "running"
        || backend_profiles::current(tx, org, instance).await?.as_ref()
            != Some(&plan["backend_profile"])
    {
        return Err(conflict());
    }
    let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_operations WHERE instance_id=$1 AND state IN ('queued','running','reconciling')) OR EXISTS(SELECT 1 FROM management_environment_jobs WHERE instance_id=$1 AND state IN ('queued','running','held')) OR EXISTS(SELECT 1 FROM management_vm_jobs WHERE instance_id=$1 AND state IN ('queued','running','held'))").bind(instance).fetch_one(&mut **tx).await?;
    if busy || rollouts::reserved(tx, instance).await? {
        return Err(conflict());
    }
    environments::admission(tx, &current, None).await?;
    let policy: PolicyRevision = serde_json::from_value(current["policy"].clone())?;
    host_capacity::require_admission(
        tx,
        org,
        serde_json::from_value(current["executor_host_id"].clone())?,
        instance,
        serde_json::from_value(current["execution_epoch"].clone())?,
        &policy,
    )
    .await?;
    let task = &plan["request"]["task_input"];
    if matches!(
        task["operation"].as_str(),
        Some("process" | "file_read" | "file_write")
    ) {
        let request = &plan["request"];
        let (body, path) = if task["operation"] == "process" {
            let workflow: Uuid = serde_json::from_value(request["workflow_id"].clone())?;
            (
                json!({"organisation_id":org,"instance_id":instance,"execution_epoch":request["execution_epoch"],"user_id":user,"management_session_id":request["management_session_id"],"environment_id":request["environment_id"],"workflow_id":request["workflow_id"],"step_id":request["step_id"],"request_id":request["request_id"]}),
                format!("/api/v1/enterprise/workflows/{workflow}/environment-authority"),
            )
        } else {
            let transfer: Uuid = serde_json::from_value(task["transfer_request_id"].clone())?;
            (
                json!({"organisation_id":org,"instance_id":instance,"execution_epoch":request["execution_epoch"],"user_id":user,"management_session_id":request["management_session_id"],"environment_id":request["environment_id"],"transfer_request_id":task["transfer_request_id"],"transfer_grant_id":task["transfer_grant_id"],"expected_transfer_revision":task["expected_transfer_revision"],"operation":task["operation"],"relative_path":task["relative_path"]}),
                format!("/api/v1/enterprise/transfers/{transfer}/environment-authority"),
            )
        };
        let mut url = crate::instance_gateway::origin(instance)?;
        if url.scheme() != "https" {
            return Err(conflict());
        }
        url.set_path(&path);
        let key = crate::fleet::current_instance_key(tx, org, instance).await?;
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15));
        if let Some(path) = std::env::var_os("COWORK_INSTANCE_GATEWAY_CA_FILE") {
            let pem = std::fs::read(path).map_err(|e| ApiError::Internal(e.into()))?;
            builder = builder.add_root_certificate(
                reqwest::Certificate::from_pem(&pem).map_err(|e| ApiError::Internal(e.into()))?,
            );
        }
        let response = builder
            .build()
            .map_err(|e| ApiError::Internal(e.into()))?
            .post(url)
            .header("x-cowork-instance-key", key)
            .json(&body)
            .send()
            .await
            .map_err(|_| {
                ApiError::Unavailable("current original transfer authority unavailable".into())
            })?;
        if !response.status().is_success() || response.content_length().is_some_and(|n| n > 16384) {
            return Err(conflict());
        }
        let bytes = response.bytes().await.map_err(|_| conflict())?;
        if bytes.len() > 16384 {
            return Err(conflict());
        }
        let reply: Value = serde_json::from_slice(&bytes)?;
        if reply["authorized"] != true
            || body
                .as_object()
                .is_none_or(|fields| fields.iter().any(|(k, v)| reply[k] != *v))
        {
            return Err(conflict());
        }
    }
    Ok(())
}
fn supported(v: &Value, capabilities: &[String]) -> bool {
    let Some(o) = v.as_object() else { return false };
    if matches!(v["operation"].as_str(), Some("file_read" | "file_write")) {
        return o.keys().all(|k| {
            matches!(
                k.as_str(),
                "operation"
                    | "relative_path"
                    | "transfer_request_id"
                    | "transfer_grant_id"
                    | "expected_transfer_revision"
                    | "expected_sha256"
                    | "content_base64"
            )
        }) && v["relative_path"].as_str().is_some_and(|p| {
            p.len() <= 1024
                && p.split('/').count() <= 32
                && p.split('/').all(|s| {
                    !s.is_empty()
                        && s.len() <= 128
                        && s != "."
                        && s != ".."
                        && s.as_bytes()[0].is_ascii_alphanumeric()
                        && s.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._ -".contains(&b))
                })
        }) && ["transfer_request_id", "transfer_grant_id"]
            .iter()
            .all(|k| {
                v[*k]
                    .as_str()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .is_some_and(|u| !u.is_nil())
            })
            && v["expected_transfer_revision"]
                .as_i64()
                .is_some_and(|n| n > 0)
            && (v["expected_sha256"].is_null() || v["expected_sha256"].as_str().is_some_and(sha))
            && if v["operation"] == "file_read" {
                capabilities == ["file.read".to_string()] && !o.contains_key("content_base64")
            } else {
                capabilities == ["file.write".to_string()]
                    && v["content_base64"].as_str().is_some_and(bounded_base64)
            };
    }
    o.keys().all(|k| {
        matches!(
            k.as_str(),
            "operation" | "argv" | "stdin_base64" | "timeout_seconds"
        )
    }) && v["operation"] == "process"
        && v["argv"].as_array().is_some_and(|a| {
            !a.is_empty()
                && a.len() <= 64
                && a.iter().all(|v| {
                    v.as_str()
                        .is_some_and(|s| !s.is_empty() && s.len() <= 8192 && !s.contains('\0'))
                })
        })
        && v["timeout_seconds"]
            .as_u64()
            .is_some_and(|n| (1..=300).contains(&n))
        && v.get("stdin_base64").is_none_or(|s| {
            s.as_str().is_some_and(|s| {
                s.len() <= 349528
                    && base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s).is_ok()
            })
        })
        && capabilities == ["process.execute".to_string()]
}
fn sha(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn bounded_base64(v: &str) -> bool {
    v.len() <= 349528
        && base64::Engine::decode(&base64::engine::general_purpose::STANDARD, v)
            .is_ok_and(|raw| raw.len() <= 262144)
}
fn public(row: &Value) -> Value {
    let mut v = row.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("plan");
        o.remove("lease_token");
    }
    v
}
pub(super) async fn submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Result<Json<Value>, ApiError> {
    if request.format != 1
        || [
            request.request_id,
            request.workflow_id,
            request.step_id,
            request.user_id,
            request.management_session_id,
            request.environment_id,
        ]
        .iter()
        .any(Uuid::is_nil)
        || request.expected_environment_revision < 1
        || serde_json::to_vec(&request)?.len() > 524288
    {
        return Err(invalid("bounded original environment task required"));
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    crate::fleet::verify_instance_key(
        &mut tx,
        &headers,
        request.organisation_id,
        request.instance_id,
    )
    .await?;
    // Check before old-request lookup as well as in the frozen plan authority:
    // a previously admitted request cannot replay after environment revocation.
    environment_access::require(
        &mut tx,
        request.organisation_id,
        request.instance_id,
        request.environment_id,
        request.user_id,
        environment_access::Role::Runner,
    )
    .await?;
    let digest = hash(&request)?;
    if let Some(row)=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM management_environment_executions e WHERE instance_id=$1 AND request_id=$2").bind(request.instance_id).bind(request.request_id).fetch_optional(&mut *tx).await?{
        if row["request_sha256"]!=digest || row["user_id"]!=json!(request.user_id){return Err(conflict());}authority(&state,&mut tx,&row["plan"]).await?;return Ok(Json(public(&row)));
    }
    let principal = Principal {
        user_id: request.user_id,
        session_id: Some(request.management_session_id),
        bootstrap: false,
    };
    let actor =
        job_authority::capture(&state, &mut tx, &principal, request.organisation_id).await?;
    let instance:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 AND organisation_id=$2 FOR SHARE").bind(request.instance_id).bind(request.organisation_id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    let env:Value=sqlx::query_scalar("SELECT to_jsonb(e) FROM management_environments e WHERE id=$1 AND instance_id=$2 AND organisation_id=$3 FOR UPDATE").bind(request.environment_id).bind(request.instance_id).bind(request.organisation_id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    let profile = backend_profiles::current(&mut tx, request.organisation_id, request.instance_id)
        .await?
        .ok_or_else(conflict)?;
    if instance["execution_epoch"] != json!(request.execution_epoch)
        || env["revision"] != request.expected_environment_revision
        || profile["specification_sha256"] != request.profile_sha256
    {
        return Err(conflict());
    }
    let template:Value=sqlx::query_scalar("SELECT to_jsonb(t) FROM management_templates t WHERE id=$1 AND organisation_id=$2 AND revision=$3 FOR SHARE").bind(serde_json::from_value::<Uuid>(env["template_id"].clone())?).bind(request.organisation_id).bind(env["template_revision"].as_i64().ok_or_else(conflict)?).fetch_one(&mut *tx).await?;
    let controller = vm_controllers::current(
        &mut tx,
        request.organisation_id,
        request.instance_id,
        template["backend"].as_str().ok_or_else(conflict)?,
    )
    .await?
    .unwrap_or(Value::Null);
    let id = Uuid::new_v4();
    let plan = json!({"format":1,"id":id,"organisation_id":request.organisation_id,"instance_id":request.instance_id,"user_id":request.user_id,"actor_authority":actor,"instance":instance,"environment":env,"template":template,"environment_controller":controller,"backend_profile":profile,"executor_host_binding":profile["executor_host_binding"],"request":request});
    authority(&state, &mut tx, &plan).await?;
    let executable = supported(&request.task_input, &request.capabilities);
    let state_name = if executable { "queued" } else { "halted" };
    if executable {
        execution_admission::auxiliary_admission(
            &mut tx,
            request.organisation_id,
            request.instance_id,
            request.user_id,
            serde_json::from_value::<PolicyRevision>(instance["policy"].clone())?
                .limits
                .concurrent_runs,
        )
        .await?;
    }
    let result = if executable {
        Value::Null
    } else {
        json!({"format":1,"state":"halted","error_code":"environment_task_runtime_operation_unavailable"})
    };
    sqlx::query("INSERT INTO management_environment_executions(id,organisation_id,instance_id,environment_id,user_id,request_id,request_sha256,plan,plan_sha256,state,result,workflow_id,step_id,completed_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,CASE WHEN $10='halted' THEN now() ELSE NULL END)").bind(id).bind(request.organisation_id).bind(request.instance_id).bind(request.environment_id).bind(request.user_id).bind(request.request_id).bind(&digest).bind(&plan).bind(hash(&plan)?).bind(state_name).bind(&result).bind(request.workflow_id).bind(request.step_id).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        request.organisation_id,
        request.user_id,
        "environment.task.admit",
        id,
        json!({"request_sha256":digest,"state":state_name,"environment_id":request.environment_id}),
    )
    .await?;
    let row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(e) FROM management_environment_executions e WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(public(&row)))
}
pub(super) async fn inspect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request): Path<Uuid>,
    Query(scope): Query<Read>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    crate::fleet::verify_instance_key(&mut tx, &headers, scope.organisation_id, scope.instance_id)
        .await?;
    let row:Value=sqlx::query_scalar("SELECT to_jsonb(e) FROM management_environment_executions e WHERE organisation_id=$1 AND instance_id=$2 AND request_id=$3 AND user_id=$4").bind(scope.organisation_id).bind(scope.instance_id).bind(request).bind(scope.user_id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    let reader = Principal {
        user_id: scope.user_id,
        session_id: Some(scope.management_session_id),
        bootstrap: false,
    };
    // A fresh current reader may inspect the unchanged old receipt. Its login
    // never replaces the immutable original dispatch authority in the plan.
    job_authority::capture(&state, &mut tx, &reader, scope.organisation_id).await?;
    environment_access::require(
        &mut tx,
        scope.organisation_id,
        scope.instance_id,
        serde_json::from_value(row["environment_id"].clone())?,
        scope.user_id,
        environment_access::Role::Viewer,
    )
    .await?;
    if !crate::fleet::allowed_on(
        &mut *tx,
        &crate::fleet::AccessRequest {
            organisation_id: scope.organisation_id,
            instance_id: scope.instance_id,
            user_id: scope.user_id,
        },
    )
    .await?
    {
        return Err(denied());
    }
    tx.commit().await?;
    Ok(Json(public(&row)))
}
pub(super) async fn stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request): Path<Uuid>,
    Query(scope): Query<Read>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    crate::fleet::verify_instance_key(&mut tx, &headers, scope.organisation_id, scope.instance_id)
        .await?;
    let reader = Principal {
        user_id: scope.user_id,
        session_id: Some(scope.management_session_id),
        bootstrap: false,
    };
    let auth = job_authority::capture(&state, &mut tx, &reader, scope.organisation_id).await?;
    if !crate::fleet::allowed_on(
        &mut *tx,
        &crate::fleet::AccessRequest {
            organisation_id: scope.organisation_id,
            instance_id: scope.instance_id,
            user_id: scope.user_id,
        },
    )
    .await?
    {
        return Err(denied());
    }
    let row:Value=sqlx::query_scalar("SELECT to_jsonb(e) FROM management_environment_executions e WHERE organisation_id=$1 AND instance_id=$2 AND request_id=$3 AND user_id=$4 FOR UPDATE").bind(scope.organisation_id).bind(scope.instance_id).bind(request).bind(scope.user_id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    let next = if row["state"] == "queued" && row["lease_token"].is_null() {
        "halted"
    } else if matches!(row["state"].as_str(), Some("running" | "held")) {
        "held"
    } else {
        row["state"].as_str().ok_or_else(conflict)?
    };
    // A claimed command has no trusted PID/ExecID retirement witness yet.
    // Stop blocks every new IO authorization, but never frees its capacity.
    let result = if next == "halted" {
        json!({"format":1,"state":"halted","original_dispatch_absent":true,"stop_verified":true})
    } else {
        row["result"].clone()
    };
    sqlx::query("UPDATE management_environment_executions SET state=$2,stop_requested_at=COALESCE(stop_requested_at,now()),stop_reader_authority=COALESCE(stop_reader_authority,$3),result=$4,completed_at=CASE WHEN $2='halted' THEN COALESCE(completed_at,now()) ELSE completed_at END WHERE id=$1").bind(serde_json::from_value::<Uuid>(row["id"].clone())?).bind(next).bind(auth).bind(result).execute(&mut *tx).await?;
    let value: Value = sqlx::query_scalar(
        "SELECT to_jsonb(e) FROM management_environment_executions e WHERE id=$1",
    )
    .bind(serde_json::from_value::<Uuid>(row["id"].clone())?)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(public(&value)))
}
pub(super) async fn destinations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(scope): Query<Read>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let mut tx = state.pool.begin().await?;
    crate::fleet::verify_instance_key(&mut tx, &headers, scope.organisation_id, scope.instance_id)
        .await?;
    let reader = Principal {
        user_id: scope.user_id,
        session_id: Some(scope.management_session_id),
        bootstrap: false,
    };
    job_authority::capture(&state, &mut tx, &reader, scope.organisation_id).await?;
    if !crate::fleet::allowed_on(
        &mut *tx,
        &crate::fleet::AccessRequest {
            organisation_id: scope.organisation_id,
            instance_id: scope.instance_id,
            user_id: scope.user_id,
        },
    )
    .await?
    {
        return Err(denied());
    }
    let profile = backend_profiles::current(&mut tx, scope.organisation_id, scope.instance_id)
        .await?
        .ok_or_else(conflict)?;
    let i:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 AND organisation_id=$2 FOR SHARE").bind(scope.instance_id).bind(scope.organisation_id).fetch_one(&mut *tx).await?;
    host_capacity::require_admission(
        &mut tx,
        scope.organisation_id,
        serde_json::from_value(i["executor_host_id"].clone())?,
        scope.instance_id,
        serde_json::from_value(i["execution_epoch"].clone())?,
        &serde_json::from_value(i["policy"].clone())?,
    )
    .await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(e) FROM management_environments e WHERE organisation_id=$1 AND instance_id=$2 AND state='running' AND runtime IS NOT NULL AND management_environment_access_allowed(organisation_id,instance_id,id,$3,'runner') ORDER BY created_at LIMIT 200").bind(scope.organisation_id).bind(scope.instance_id).bind(scope.user_id).fetch_all(&mut *tx).await?;
    let mut result = Vec::new();
    for env in rows {
        let controller = vm_controllers::current(
            &mut tx,
            scope.organisation_id,
            scope.instance_id,
            env["spec"]["backend"].as_str().ok_or_else(conflict)?,
        )
        .await?
        .unwrap_or(Value::Null);
        let fingerprint = vm_controllers::fingerprint(&profile, &controller)?;
        let qualified:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_template_qualifications WHERE organisation_id=$1 AND instance_id=$2 AND template_id=$3 AND template_revision=$4 AND profile_sha256=$5)").bind(scope.organisation_id).bind(scope.instance_id).bind(serde_json::from_value::<Uuid>(env["template_id"].clone())?).bind(env["template_revision"].as_i64()).bind(fingerprint).fetch_one(&mut *tx).await?;
        if qualified
            && (controller.is_null() && env["runtime"]["vm_controller_id"].is_null()
                || !controller.is_null()
                    && env["runtime"]["vm_controller_id"] == controller["id"]
                    && env["runtime"]["controller_sha256"] == controller["specification_sha256"])
        {
            result.push(json!({"id":env["id"],"name":env["spec"]["name"],"revision":env["revision"],"profile_sha256":profile["specification_sha256"]}));
        }
    }
    tx.commit().await?;
    Ok(Json(result))
}
pub(super) async fn claim(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Operator>,
) -> Result<Json<Value>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    let row:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(e) FROM management_environment_executions e WHERE state='queued' AND plan->'executor_host_binding'=$1 ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1").bind(serde_json::to_value(request.executor_host_binding)?).fetch_optional(&mut *tx).await?;
    let Some(mut row) = row else {
        return Ok(Json(Value::Null));
    };
    authority(&state, &mut tx, &row["plan"]).await?;
    let lease = Uuid::new_v4();
    sqlx::query(
        "UPDATE management_environment_executions SET state='running',lease_token=$2 WHERE id=$1",
    )
    .bind(serde_json::from_value::<Uuid>(row["id"].clone())?)
    .bind(lease)
    .execute(&mut *tx)
    .await?;
    row["state"] = json!("running");
    row["lease_token"] = json!(lease);
    tx.commit().await?;
    Ok(Json(row))
}
pub(super) async fn pending(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Operator>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    let rows=sqlx::query_scalar("SELECT to_jsonb(e) FROM management_environment_executions e WHERE state IN ('running','held') AND plan->'executor_host_binding'=$1 ORDER BY created_at LIMIT 100").bind(serde_json::to_value(request.executor_host_binding)?).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(rows))
}
async fn bound<'a>(
    state: &'a AppState,
    headers: &HeaderMap,
    id: Uuid,
    request: &Completion,
) -> Result<(Transaction<'a, Postgres>, Value), ApiError> {
    let operator = executor_hosts::authenticate(state, headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    let row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(e) FROM management_environment_executions e WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    operator.verify_operation(&row["plan"]["executor_host_binding"])?;
    if row["lease_token"] != json!(request.lease_token) || row["plan_sha256"] != request.plan_sha256
    {
        return Err(conflict());
    }
    Ok((tx, row))
}
pub(super) async fn authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Completion>,
) -> Result<Json<Value>, ApiError> {
    let (mut tx, row) = bound(&state, &headers, id, &request).await?;
    if row["state"] != "running" || !row["stop_requested_at"].is_null() {
        return Err(conflict());
    }
    authority(&state, &mut tx, &row["plan"]).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":id,"plan_sha256":request.plan_sha256,"authorized":true}),
    ))
}
pub(super) async fn hold(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Completion>,
) -> Result<Json<Value>, ApiError> {
    let (mut tx, row) = bound(&state, &headers, id, &request).await?;
    if !matches!(row["state"].as_str(), Some("running" | "held")) {
        return Err(conflict());
    }
    sqlx::query("UPDATE management_environment_executions SET state='held' WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":"held"})))
}
pub(super) async fn complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Completion>,
) -> Result<Json<Value>, ApiError> {
    let (mut tx, row) = bound(&state, &headers, id, &request).await?;
    if matches!(row["state"].as_str(), Some("completed" | "failed")) {
        if row["result"] != request.result {
            return Err(conflict());
        }
        return Ok(Json(public(&row)));
    }
    if !matches!(row["state"].as_str(), Some("running" | "held")) {
        return Err(conflict());
    }
    let result = &request.result;
    if result["format"] != 1
        || result["job_id"] != json!(id)
        || result["plan_sha256"] != request.plan_sha256
        || result["runtime"] != row["plan"]["environment"]["runtime"]
        || result["original_runtime_verified"] != true
        || result["delivery_complete"] != true
        || result["exit_code"]
            .as_i64()
            .is_none_or(|v| !(0..=255).contains(&v))
    {
        return Err(conflict());
    }
    for key in ["stdout_base64", "stderr_base64"] {
        if result[key].as_str().is_none_or(|v| {
            v.len() > 349528
                || base64::Engine::decode(&base64::engine::general_purpose::STANDARD, v).is_err()
        }) {
            return Err(conflict());
        }
    }
    let task = &row["plan"]["request"]["task_input"];
    if matches!(task["operation"].as_str(), Some("file_read" | "file_write")) {
        let file = &result["file_result"];
        let mut original_file = task.clone();
        original_file["request_id"] = row["plan"]["request"]["request_id"].clone();
        if file["format"] != 1
            || [
                "operation",
                "relative_path",
                "transfer_request_id",
                "transfer_grant_id",
                "expected_transfer_revision",
            ]
            .iter()
            .any(|k| file[*k] != task[*k])
            || file["size"].as_u64().is_none_or(|v| v > 262144)
            || file["sha256"].as_str().is_none_or(|v| !sha(v))
            || file["local_receipt"]["content_sha256"] != file["sha256"]
            || file["local_receipt"]["request_sha256"] != hash(&original_file)?
        {
            return Err(conflict());
        }
        if task["operation"] == "file_read" {
            let data = file["content_base64"].as_str().ok_or_else(conflict)?;
            if !bounded_base64(data) {
                return Err(conflict());
            }
            let raw = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
                .map_err(|_| conflict())?;
            if file["size"] != json!(raw.len())
                || file["sha256"] != hex::encode(Sha256::digest(&raw))
            {
                return Err(conflict());
            }
        } else if file["conflict"] == false {
            if file["destination_before_sha256"] != task["expected_sha256"]
                || file["destination_after_sha256"] != file["sha256"]
                || file["local_receipt"]["kernel_write_lease"] != true
            {
                return Err(conflict());
            }
        } else if file["conflict"] != true || file["conflict_relative_path"].as_str().is_none() {
            return Err(conflict());
        }
    }
    let next = if result["exit_code"] == 0 {
        "completed"
    } else {
        "failed"
    };
    sqlx::query("UPDATE management_environment_executions SET state=$2,result=$3,completed_at=now() WHERE id=$1").bind(id).bind(next).bind(result).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        serde_json::from_value(row["organisation_id"].clone())?,
        serde_json::from_value(row["user_id"].clone())?,
        "environment.task.complete",
        id,
        json!({"state":next,"result_sha256":hash(result)?}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":next,"result":result})))
}
