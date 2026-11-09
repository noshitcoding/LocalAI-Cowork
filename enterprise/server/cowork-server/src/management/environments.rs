//! Original plans and held reservations for actual template/environment runtimes.
use super::*;

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    request_id: Uuid,
    expected_instance_revision: i64,
    #[serde(default)]
    expected_environment_revision: i64,
    #[serde(default)]
    template_revision: i64,
    action: String,
    #[serde(default)]
    confirmation_name: Option<String>,
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
    receipt: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recovery {
    plan_sha256: String,
    confirmation: String,
}
fn conflict() -> ApiError {
    ApiError::Conflict("original environment authority, runtime or reservation changed".into())
}
fn hash(value: &impl serde::Serialize) -> Result<String, ApiError> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}
pub(super) async fn instance_unlocked(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<(), ApiError> {
    environment_execution::unlocked(tx, id).await?;
    let occupied: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_environments WHERE instance_id=$1 AND state<>'deleted') OR EXISTS(SELECT 1 FROM management_environment_jobs WHERE instance_id=$1 AND state IN ('queued','running','held'))")
        .bind(id).fetch_one(&mut **tx).await?;
    if occupied {
        return Err(conflict());
    }
    Ok(())
}
pub(super) async fn execution_unlocked(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<(), ApiError> {
    let occupied: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_environments WHERE instance_id=$1 AND state IN ('starting','running','stopping','deleting','held')) OR EXISTS(SELECT 1 FROM management_environment_jobs WHERE instance_id=$1 AND state IN ('queued','running','held'))")
        .bind(id).fetch_one(&mut **tx).await?;
    if occupied {
        return Err(ApiError::Unavailable("environment_pool_reserved".into()));
    }
    Ok(())
}
async fn current(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    id: Uuid,
) -> Result<(Value, Value), ApiError> {
    let instance:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE organisation_id=$1 AND id=$2 AND state='ready' FOR UPDATE")
        .bind(org).bind(id).fetch_optional(&mut **tx).await?.ok_or_else(conflict)?;
    let profile = backend_profiles::current(tx, org, id)
        .await?
        .ok_or_else(conflict)?;
    let policy: PolicyRevision = serde_json::from_value(instance["policy"].clone())?;
    host_capacity::require_admission(
        tx,
        org,
        serde_json::from_value(instance["executor_host_id"].clone())?,
        id,
        serde_json::from_value(instance["execution_epoch"].clone())?,
        &policy,
    )
    .await?;
    Ok((instance, profile))
}
fn supported(template: &Value, profile: &Value, controller: &Value) -> Result<(), ApiError> {
    let spec = &profile["specification"];
    if !(template["backend"] == "docker"
        && template["isolation"] == "virtual_machine"
        && spec["kind"] == "vm_compose"
        || template["backend"] == "kubernetes"
            && template["isolation"] == "container"
            && spec["kind"] == "kubernetes"
        || !controller.is_null()
            && controller["backend"] == template["backend"]
            && template["isolation"] == "virtual_machine"
            && spec["kind"] == "vm_compose")
    {
        return Err(conflict());
    }
    Ok(())
}
async fn idle(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<(), ApiError> {
    environment_execution::unlocked(tx, id).await?;
    vm_snapshots::unlocked(tx, id).await?;
    let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_operations WHERE instance_id=$1 AND state IN ('queued','running','reconciling')) OR EXISTS(SELECT 1 FROM management_execution_reservations WHERE instance_id=$1 AND state='held') OR EXISTS(SELECT 1 FROM management_environment_jobs WHERE instance_id=$1 AND state IN ('queued','running','held'))")
        .bind(id).fetch_one(&mut **tx).await?;
    if busy || rollouts::reserved(tx, id).await? {
        return Err(conflict());
    }
    Ok(())
}
pub(super) async fn admission(
    tx: &mut Transaction<'_, Postgres>,
    instance: &Value,
    candidate: Option<&EnvironmentSpec>,
) -> Result<(), ApiError> {
    super::disk_quotas::environment_admission(tx, instance, candidate).await?;
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT spec FROM management_environments WHERE instance_id=$1 AND state<>'deleted'",
    )
    .bind(serde_json::from_value::<Uuid>(instance["id"].clone())?)
    .fetch_all(&mut **tx)
    .await?;
    let org_policy: Value =
        sqlx::query_scalar("SELECT policy FROM management_organisations WHERE id=$1")
            .bind(serde_json::from_value::<Uuid>(
                instance["organisation_id"].clone(),
            )?)
            .fetch_one(&mut **tx)
            .await?;
    let effective = serde_json::from_value::<PolicyRevision>(instance["policy"].clone())?
        .restricted_by(&serde_json::from_value(org_policy)?)
        .restricted_by(platform_ceiling());
    let mut amounts = [0u64; 4];
    for spec in rows
        .iter()
        .map(|v| serde_json::from_value::<EnvironmentSpec>(v.clone()))
        .chain(candidate.map(|v| Ok(v.clone())))
    {
        let spec = spec?;
        if !spec.limits.fits(&effective.limits) || spec.isolation < effective.minimum_isolation {
            return Err(conflict());
        }
        for (total, value) in amounts.iter_mut().zip([
            spec.limits.cpu_millis,
            spec.limits.memory_mib,
            spec.limits.storage_gib,
            1,
        ]) {
            *total += u64::from(value);
        }
    }
    if amounts
        .iter()
        .zip([
            effective.limits.cpu_millis,
            effective.limits.memory_mib,
            effective.limits.storage_gib,
            effective.limits.concurrent_runs,
        ])
        .any(|(used, max)| *used > u64::from(max))
    {
        return Err(ApiError::Unavailable("environment_pool_exhausted".into()));
    }
    Ok(())
}
pub(super) async fn create(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let request_sha256 = hash(&body)?;
    let request_id: Option<Uuid> = body
        .as_object_mut()
        .ok_or_else(conflict)?
        .remove("request_id")
        .map(serde_json::from_value)
        .transpose()?;
    if request_id.is_some_and(|v| v.is_nil()) {
        return Err(conflict());
    }
    let spec: EnvironmentSpec = serde_json::from_value(body)?;
    instance_access(&state, &principal, org, id, true).await?;
    name(&spec.name)?;
    spec.limits.validate().map_err(invalid)?;
    if spec.instance_id != id || spec.template_revision == 0 {
        return Err(conflict());
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    instance_members::authorize(&mut tx, &principal, org, id).await?;
    if let Some(request_id) = request_id {
        if let Some(prior)=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM management_environments e WHERE organisation_id=$1 AND registered_by=$2 AND request_id=$3")
            .bind(org).bind(principal.user_id).bind(request_id).fetch_optional(&mut *tx).await? {
            if prior["request_sha256"]!=request_sha256 || prior["instance_id"]!=json!(id) {return Err(conflict());}
            return Ok(Json(json!({"id":prior["id"],"state":prior["state"],"revision":prior["revision"]})));
        }
    }
    let (instance, profile) = current(&mut tx, org, id).await?;
    idle(&mut tx, id).await?;
    let template:Value=sqlx::query_scalar("SELECT to_jsonb(t) FROM management_templates t WHERE organisation_id=$1 AND id=$2 AND revision=$3 AND enabled FOR SHARE")
        .bind(org).bind(spec.template_id).bind(i64::try_from(spec.template_revision).map_err(|_|conflict())?).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?;
    let controller = vm_controllers::current(
        &mut tx,
        org,
        id,
        template["backend"].as_str().ok_or_else(conflict)?,
    )
    .await?
    .unwrap_or(Value::Null);
    supported(&template, &profile, &controller)?;
    if template["backend"] != serde_json::to_value(spec.backend)?
        || template["isolation"] != serde_json::to_value(spec.isolation)?
    {
        return Err(conflict());
    }
    if !controller.is_null() {
        for (key, v) in [
            ("cpu_millis", spec.limits.cpu_millis),
            ("memory_mib", spec.limits.memory_mib),
            ("storage_gib", spec.limits.storage_gib),
        ] {
            if controller["specification"]["limits"][key]
                .as_u64()
                .is_none_or(|max| u64::from(v) > max)
            {
                return Err(conflict());
            }
        }
    }
    let qualified:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_template_qualifications WHERE organisation_id=$1 AND template_id=$2 AND template_revision=$3 AND instance_id=$4 AND profile_sha256=$5)")
        .bind(org).bind(spec.template_id).bind(spec.template_revision as i64).bind(id).bind(vm_controllers::fingerprint(&profile,&controller)?).fetch_one(&mut *tx).await?;
    if !qualified {
        return Err(invalid(
            "qualify this original template on this assigned physical profile",
        ));
    }
    admission(&mut tx, &instance, Some(&spec)).await?;
    let env = Uuid::new_v4();
    sqlx::query("INSERT INTO management_environments(id,organisation_id,instance_id,template_id,template_revision,spec,registered_by,request_id,request_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(env).bind(org).bind(id).bind(spec.template_id).bind(spec.template_revision as i64).bind(serde_json::to_value(spec)?).bind(principal.user_id).bind(request_id).bind(request_sha256).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "environment.create",
        env,
        json!({"profile_sha256":profile["specification_sha256"]}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":env,"state":"pending","revision":1})))
}
pub(super) async fn qualify(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance, template)): Path<(Uuid, Uuid, Uuid)>,
    Json(request): Json<Request>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    if request.action != "qualify" {
        return Err(conflict());
    }
    prepare(
        &state,
        &principal,
        org,
        instance,
        None,
        Some(template),
        request,
    )
    .await
    .map(Json)
}
pub(super) async fn action(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance, env)): Path<(Uuid, Uuid, Uuid)>,
    Json(request): Json<Request>,
) -> Result<Json<Value>, ApiError> {
    instance_access(&state, &principal, org, instance, true).await?;
    if !matches!(
        request.action.as_str(),
        "start" | "stop" | "delete" | "inspect"
    ) {
        return Err(conflict());
    }
    prepare(&state, &principal, org, instance, Some(env), None, request)
        .await
        .map(Json)
}
async fn prepare(
    state: &AppState,
    principal: &Principal,
    org: Uuid,
    id: Uuid,
    env_id: Option<Uuid>,
    template_id: Option<Uuid>,
    request: Request,
) -> Result<Value, ApiError> {
    if request.request_id.is_nil() {
        return Err(conflict());
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    instance_members::authorize(&mut tx, principal, org, id).await?;
    if request.action == "qualify" {
        sso::authorize(&mut tx, org, principal).await?;
    }
    let digest = hash(
        &json!({"instance":id,"environment":env_id,"template":template_id,"request":request}),
    )?;
    if let Some(prior)=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(j)-'lease_token'-'plan' FROM management_environment_jobs j WHERE organisation_id=$1 AND actor_user_id=$2 AND request_id=$3")
        .bind(org).bind(principal.user_id).bind(request.request_id).fetch_optional(&mut *tx).await? {
        if prior["request_sha256"]!=digest {return Err(conflict());}return Ok(prior);
    }
    let (instance, profile) = current(&mut tx, org, id).await?;
    if instance["revision"].as_i64() != Some(request.expected_instance_revision) {
        return Err(conflict());
    }
    idle(&mut tx, id).await?;
    admission(&mut tx, &instance, None).await?;
    let env = if let Some(env) = env_id {
        sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM management_environments e WHERE id=$1 AND organisation_id=$2 AND instance_id=$3 FOR UPDATE")
        .bind(env).bind(org).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?
    } else {
        Value::Null
    };
    if env_id.is_some()
        && (env["revision"].as_i64() != Some(request.expected_environment_revision)
            || env["state"] == "deleted")
    {
        return Err(conflict());
    }
    if request.action == "delete"
        && request.confirmation_name.as_deref() != env["spec"]["name"].as_str()
    {
        return Err(invalid(
            "confirm the original environment name before permanent workspace deletion",
        ));
    }
    let template_id = template_id
        .unwrap_or(serde_json::from_value(env["template_id"].clone()).unwrap_or(Uuid::nil()));
    let revision = if env_id.is_some() {
        env["template_revision"].as_i64().ok_or_else(conflict)?
    } else {
        request.template_revision
    };
    let template:Value=sqlx::query_scalar("SELECT to_jsonb(t) FROM management_templates t WHERE organisation_id=$1 AND id=$2 AND revision=$3 FOR SHARE")
        .bind(org).bind(template_id).bind(revision).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?;
    let controller = vm_controllers::current(
        &mut tx,
        org,
        id,
        template["backend"].as_str().ok_or_else(conflict)?,
    )
    .await?
    .unwrap_or(Value::Null);
    supported(&template, &profile, &controller)?;
    if request.action == "start" && !(env["state"] == "pending" || env["state"] == "stopped") {
        return Err(conflict());
    }
    if request.action == "qualify" {
        // The probe occupies the same sandbox pool. Retained env allocations are
        // conservatively reserved and must leave a complete bounded probe slot.
        let probe = EnvironmentSpec {
            instance_id: id,
            name: "qualification".into(),
            backend: serde_json::from_value(template["backend"].clone())?,
            isolation: serde_json::from_value(template["isolation"].clone())?,
            template_id,
            template_revision: revision as u64,
            persistent: false,
            shared_workspace: false,
            limits: ResourceLimits {
                cpu_millis: if template["backend"] == "open_shell" {
                    500
                } else {
                    250
                },
                memory_mib: if template["backend"] == "open_shell" {
                    512
                } else {
                    128
                },
                storage_gib: 1,
                concurrent_runs: 1,
                instances: 1,
            },
        };
        admission(&mut tx, &instance, Some(&probe)).await?;
    }
    let job = Uuid::new_v4();
    let challenge = Uuid::new_v4();
    let actor_authority = job_authority::capture(state, &mut tx, principal, org).await?;
    let plan = json!({"format":1,"id":job,"organisation_id":org,"instance_id":id,"actor_user_id":principal.user_id,"actor_authority":actor_authority,"action":request.action,"challenge":challenge,"instance":instance,"backend_profile":profile,"executor_host_binding":profile["executor_host_binding"],"environment":env,"template":template,"environment_controller":controller});
    let sha = hash(&plan)?;
    sqlx::query("INSERT INTO management_environment_jobs(id,organisation_id,instance_id,environment_id,template_id,template_revision,actor_user_id,request_id,request_sha256,action,plan,plan_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
        .bind(job).bind(org).bind(id).bind(env_id).bind(template_id).bind(revision).bind(principal.user_id).bind(request.request_id).bind(digest).bind(&request.action).bind(plan).bind(&sha).execute(&mut *tx).await?;
    if let Some(env) = env_id {
        if request.action != "inspect" {
            let next = match request.action.as_str() {
                "start" => "starting",
                "stop" => "stopping",
                _ => "deleting",
            };
            sqlx::query("UPDATE management_environments SET state=$2 WHERE id=$1")
                .bind(env)
                .bind(next)
                .execute(&mut *tx)
                .await?;
        }
    }
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "environment.plan",
        job,
        json!({"action":request.action,"environment_id":env_id,"plan_sha256":sha}),
    )
    .await?;
    tx.commit().await?;
    Ok(json!({"id":job,"state":"queued","plan_sha256":sha}))
}
async fn authority(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    plan: &Value,
) -> Result<(), ApiError> {
    let org = serde_json::from_value::<Uuid>(plan["organisation_id"].clone())?;
    let id = serde_json::from_value::<Uuid>(plan["instance_id"].clone())?;
    let principal = Principal {
        user_id: serde_json::from_value(plan["actor_user_id"].clone())?,
        session_id: None,
        bootstrap: false,
    };
    instance_members::authorize(tx, &principal, org, id).await?;
    if matches!(
        plan["action"].as_str(),
        Some("qualify" | "power_start" | "power_stop")
    ) {
        sso::authorize(tx, org, &principal).await?;
    }
    if plan["actor_authority"]["user_id"] != plan["actor_user_id"] {
        return Err(denied());
    }
    job_authority::validate(state, tx, &plan["actor_authority"], org).await?;
    let instance:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 AND organisation_id=$2 FOR UPDATE").bind(id).bind(org).fetch_one(&mut **tx).await?;
    if instance["revision"] != plan["instance"]["revision"]
        || instance["state"] != "ready"
        || backend_profiles::current(tx, org, id).await?.as_ref() != Some(&plan["backend_profile"])
    {
        return Err(conflict());
    }
    admission(tx, &instance, None).await?;
    if !plan["environment_controller"].is_null()
        && vm_controllers::current(
            tx,
            org,
            id,
            plan["environment_controller"]["backend"]
                .as_str()
                .ok_or_else(conflict)?,
        )
        .await?
        .as_ref()
            != Some(&plan["environment_controller"])
    {
        return Err(conflict());
    }
    Ok(())
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
    let job:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(j) FROM management_environment_jobs j WHERE state='queued' AND plan->'executor_host_binding'=$1 ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1")
        .bind(serde_json::to_value(&request.executor_host_binding)?).fetch_optional(&mut *tx).await?;
    let Some(mut job) = job else {
        return Ok(Json(Value::Null));
    };
    authority(&state, &mut tx, &job["plan"]).await?;
    let lease = Uuid::new_v4();
    sqlx::query(
        "UPDATE management_environment_jobs SET state='running',lease_token=$2 WHERE id=$1",
    )
    .bind(serde_json::from_value::<Uuid>(job["id"].clone())?)
    .bind(lease)
    .execute(&mut *tx)
    .await?;
    job["lease_token"] = json!(lease);
    job["state"] = json!("running");
    tx.commit().await?;
    Ok(Json(job))
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
    let rows=sqlx::query_scalar("SELECT to_jsonb(j) FROM management_environment_jobs j WHERE state IN ('running','held') AND plan->'executor_host_binding'=$1 ORDER BY created_at LIMIT 100")
        .bind(serde_json::to_value(&request.executor_host_binding)?).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(rows))
}
pub(super) async fn authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Completion>,
) -> Result<Json<Value>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    let row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(j) FROM management_environment_jobs j WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    operator.verify_operation(&row["plan"]["executor_host_binding"])?;
    if row["lease_token"] != json!(request.lease_token)
        || row["plan_sha256"] != request.plan_sha256
        || !matches!(row["state"].as_str(), Some("running" | "held"))
    {
        return Err(conflict());
    }
    let mut plan = row["plan"].clone();
    if row["recovery_authorized"] == true {
        plan["actor_user_id"] = row["recovery_actor_user_id"].clone();
        plan["actor_authority"] = row["recovery_authority"].clone();
    }
    authority(&state, &mut tx, &plan).await?;
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
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    let job: Value = sqlx::query_scalar(
        "SELECT to_jsonb(j) FROM management_environment_jobs j WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    operator.verify_operation(&job["plan"]["executor_host_binding"])?;
    if job["lease_token"] != json!(request.lease_token)
        || job["plan_sha256"] != request.plan_sha256
        || job["state"] == "completed"
    {
        return Err(conflict());
    }
    sqlx::query("UPDATE management_environment_jobs SET state='held' WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if !job["environment_id"].is_null() {
        sqlx::query("UPDATE management_environments SET state='held' WHERE id=$1")
            .bind(serde_json::from_value::<Uuid>(
                job["environment_id"].clone(),
            )?)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":"held"})))
}
fn receipt(plan: &Value, receipt: &Value, sha: &str) -> Result<(), ApiError> {
    if receipt["format"] != "cowork-environment-completion-v1"
        || receipt["job_id"] != plan["id"]
        || receipt["plan_sha256"] != sha
        || receipt["challenge"] != plan["challenge"]
        || receipt["backend_profile_sha256"] != plan["backend_profile"]["specification_sha256"]
        || receipt["action"] != plan["action"]
    {
        return Err(conflict());
    }
    let runtime = &receipt["runtime"];
    if matches!(plan["action"].as_str(), Some("power_start" | "power_stop")) {
        let power = &receipt["vm_power"];
        let stop = plan["action"] == "power_stop" || receipt["recovered_stopped"] == true;
        if !runtime.is_null()
            || power["format"] != "cowork-environment-vm-power-v1"
            || power["controller_id"] != plan["environment_controller"]["id"]
            || power["vm_id"] != plan["environment_controller"]["specification"]["vm_id"]
            || power["actual_running"] != !stop
            || stop
                && (power["physical_stop_verified"] != true
                    || receipt["physical_stop_verified"] != true
                    || receipt["original_runtime_absent"] != true)
        {
            return Err(conflict());
        }
        return Ok(());
    }
    if !runtime.is_null() && !plan["environment_controller"].is_null() {
        for (actual, expected) in [
            ("vm_controller_id", &plan["environment_controller"]["id"]),
            (
                "controller_sha256",
                &plan["environment_controller"]["specification_sha256"],
            ),
            ("environment_backend", &plan["template"]["backend"]),
            (
                "physical_vm_id",
                &plan["environment_controller"]["specification"]["vm_id"],
            ),
            (
                "physical_vm_hardware_uuid",
                &plan["environment_controller"]["specification"]["guest_hardware_uuid"],
            ),
        ] {
            if runtime[actual] != *expected {
                return Err(conflict());
            }
        }
    }
    if receipt["uncreated_environment_deleted"] == true {
        if plan["action"] != "delete"
            || !plan["environment"]["runtime"].is_null()
            || receipt["original_runtime_absent"] != true
            || receipt["storage_release_verified"] != true
            || !runtime.is_null()
        {
            return Err(conflict());
        }
        return Ok(());
    }
    let original_stopped = (plan["environment"]["runtime"]["stopped"] == true
        || receipt["physical_stop_verified"] == true && receipt["original_runtime_absent"] == true)
        && matches!(plan["action"].as_str(), Some("stop" | "delete" | "inspect"))
        && runtime["challenge_observed"] == plan["environment"]["runtime"]["challenge_observed"]
        && [
            "container_id",
            "sandbox_id",
            "pod_uid",
            "node_uid",
            "container_runtime_id",
            "started_at",
            "runtime_generation",
            "policy",
        ]
        .iter()
        .all(|k| runtime[*k] == plan["environment"]["runtime"][*k]);
    let recovery_cleanup = receipt["recovered_stopped"] == true
        && receipt["physical_stop_verified"] == true
        && receipt["original_runtime_absent"] == true
        && runtime["labels"]["cowork.environment-create"] == plan["id"];
    if runtime["image_reference"] != plan["template"]["image_reference"]
        || runtime["instance_id"] != plan["instance_id"]
        || runtime["organisation_id"] != plan["organisation_id"]
        || !(runtime["challenge_observed"] == plan["challenge"]
            || original_stopped
            || recovery_cleanup)
        || !recovery_cleanup && runtime["workspace_write_verified"] != true
        || !recovery_cleanup && runtime["resource_limits_verified"] != true
    {
        return Err(conflict());
    }
    match plan["backend_profile"]["specification"]["kind"].as_str() {
        Some("vm_compose") => {
            if runtime["backend"]
                != if plan["template"]["backend"] == "open_shell" {
                    "open_shell"
                } else {
                    "docker"
                }
                || runtime["daemon_id"]
                    != *if plan["environment_controller"].is_null() {
                        &plan["backend_profile"]["specification"]["daemon_id"]
                    } else {
                        &plan["environment_controller"]["specification"]["guest_daemon_id"]
                    }
                || runtime["container_id"]
                    .as_str()
                    .is_none_or(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(conflict());
            }
        }
        Some("kubernetes") => {
            if runtime["backend"] != "kubernetes"
                || runtime["cluster_uid"] != plan["backend_profile"]["specification"]["cluster_uid"]
                || runtime["namespace_uid"]
                    != plan["backend_profile"]["specification"]["namespace_uid"]
                || runtime["pod_uid"]
                    .as_str()
                    .and_then(|v| Uuid::parse_str(v).ok())
                    .is_none()
                || runtime["node_uid"]
                    .as_str()
                    .and_then(|v| Uuid::parse_str(v).ok())
                    .is_none()
                || runtime["container_runtime_id"]
                    .as_str()
                    .is_none_or(str::is_empty)
            {
                return Err(conflict());
            }
        }
        _ => return Err(conflict()),
    }
    if matches!(plan["action"].as_str(), Some("qualify" | "stop" | "delete"))
        && (receipt["physical_stop_verified"] != true || receipt["original_runtime_absent"] != true)
    {
        return Err(conflict());
    }
    if plan["action"] == "delete" && receipt["storage_release_verified"] != true {
        return Err(conflict());
    }
    if plan["action"] != "start" && plan["action"] != "qualify" {
        for key in ["container_id", "pod_uid", "labels", "storage", "started_at"] {
            if runtime[key] != plan["environment"]["runtime"][key] {
                return Err(conflict());
            }
        }
    }
    Ok(())
}
pub(super) async fn complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Completion>,
) -> Result<Json<Value>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    let job: Value = sqlx::query_scalar(
        "SELECT to_jsonb(j) FROM management_environment_jobs j WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    operator.verify_operation(&job["plan"]["executor_host_binding"])?;
    if job["lease_token"] != json!(request.lease_token) || job["plan_sha256"] != request.plan_sha256
    {
        return Err(conflict());
    }
    if job["state"] == "completed" {
        if job["receipt"] != request.receipt {
            return Err(conflict());
        }
        return Ok(Json(job["receipt"].clone()));
    }
    let mut authorized_plan = job["plan"].clone();
    if job["recovery_authorized"] == true {
        authorized_plan["actor_user_id"] = job["recovery_actor_user_id"].clone();
        authorized_plan["actor_authority"] = job["recovery_authority"].clone();
    }
    authority(&state, &mut tx, &authorized_plan).await?;
    receipt(&job["plan"], &request.receipt, &request.plan_sha256)?;
    if request.receipt["recovered_stopped"] == true
        && (job["recovery_authorized"] != true
            || request.receipt["physical_stop_verified"] != true
            || request.receipt["original_runtime_absent"] != true)
    {
        return Err(conflict());
    }
    let plan = &job["plan"];
    let org = serde_json::from_value::<Uuid>(job["organisation_id"].clone())?;
    if plan["action"] == "qualify" && request.receipt["recovered_stopped"] != true {
        sqlx::query("INSERT INTO management_template_qualifications(organisation_id,template_id,template_revision,instance_id,profile_sha256,job_id,receipt) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(template_id,template_revision,instance_id,profile_sha256) DO UPDATE SET job_id=excluded.job_id,receipt=excluded.receipt")
            .bind(org).bind(serde_json::from_value::<Uuid>(job["template_id"].clone())?).bind(job["template_revision"].as_i64()).bind(serde_json::from_value::<Uuid>(job["instance_id"].clone())?).bind(vm_controllers::fingerprint(&plan["backend_profile"],&plan["environment_controller"])?).bind(id).bind(&request.receipt).execute(&mut *tx).await?;
        sqlx::query("UPDATE management_templates SET enabled=true WHERE organisation_id=$1 AND id=$2 AND revision=$3").bind(org).bind(serde_json::from_value::<Uuid>(job["template_id"].clone())?).bind(job["template_revision"].as_i64()).execute(&mut *tx).await?;
    } else if !job["environment_id"].is_null() {
        let env_id = serde_json::from_value::<Uuid>(job["environment_id"].clone())?;
        let env: Value = sqlx::query_scalar(
            "SELECT to_jsonb(e) FROM management_environments e WHERE id=$1 FOR UPDATE",
        )
        .bind(env_id)
        .fetch_one(&mut *tx)
        .await?;
        if env["revision"] != plan["environment"]["revision"]
            || env["runtime"] != plan["environment"]["runtime"]
        {
            return Err(conflict());
        }
        let next = if request.receipt["recovered_stopped"] == true {
            "stopped"
        } else {
            match plan["action"].as_str() {
                Some("start") => "running",
                Some("stop") => "stopped",
                Some("delete") => "deleted",
                _ => env["state"].as_str().ok_or_else(conflict)?,
            }
        };
        sqlx::query("UPDATE management_environments SET state=$2,revision=revision+1,runtime=$3 WHERE id=$1").bind(env_id).bind(next).bind(&request.receipt["runtime"]).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE management_environment_jobs SET state='completed',receipt=$2,completed_at=now() WHERE id=$1").bind(id).bind(&request.receipt).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        serde_json::from_value(job["actor_user_id"].clone())?,
        "environment.complete",
        id,
        json!({"action":plan["action"],"receipt_sha256":hash(&request.receipt)?}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(request.receipt))
}
pub(super) async fn recover(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance, id)): Path<(Uuid, Uuid, Uuid)>,
    Json(request): Json<Recovery>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    if request.confirmation != "STOP_ACKNOWLEDGED_ORIGINAL_ENVIRONMENT" {
        return Err(invalid(
            "confirm physical stop of the original acknowledged runtime",
        ));
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    sso::authorize(&mut tx, org, &principal).await?;
    let job:Value=sqlx::query_scalar("SELECT to_jsonb(j) FROM management_environment_jobs j WHERE organisation_id=$1 AND instance_id=$2 AND id=$3 AND state IN ('running','held') FOR UPDATE")
        .bind(org).bind(instance).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?;
    if job["plan_sha256"] != request.plan_sha256 {
        return Err(conflict());
    }
    let mut plan = job["plan"].clone();
    plan["actor_user_id"] = json!(principal.user_id);
    let recovery_authority = job_authority::capture(&state, &mut tx, &principal, org).await?;
    plan["actor_authority"] = recovery_authority.clone();
    authority(&state, &mut tx, &plan).await?;
    sqlx::query("UPDATE management_environment_jobs SET state='held',recovery_authorized=true,recovery_actor_user_id=$2,recovery_authority=$3 WHERE id=$1").bind(id).bind(principal.user_id).bind(recovery_authority).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "environment.recovery.authorize",
        id,
        json!({"plan_sha256":request.plan_sha256}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":id,"state":"held","recovery_authorized":true}),
    ))
}
pub(super) async fn inspect_job(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance, id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    instance_access(&state, &principal, org, instance, false).await?;
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(j)-'lease_token'-'plan' FROM management_environment_jobs j WHERE organisation_id=$1 AND instance_id=$2 AND id=$3")
        .bind(org).bind(instance).bind(id).fetch_optional(&state.pool).await?.ok_or_else(denied)?))
}
pub(super) async fn jobs(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    instance_access(&state, &principal, org, instance, false).await?;
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(j)-'lease_token'-'plan' FROM management_environment_jobs j WHERE organisation_id=$1 AND instance_id=$2 ORDER BY created_at DESC LIMIT 100")
        .bind(org).bind(instance).fetch_all(&state.pool).await?))
}
