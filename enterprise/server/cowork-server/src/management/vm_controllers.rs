//! Explicit physical controller enrollment; no authority from a backend enum.
use super::*;
#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    request_id: Uuid,
    expected_instance_revision: i64,
    expected_controller_revision: i64,
    specification: Value,
    confirmation: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Activation {
    executor_host_binding: executor_hosts::HostBinding,
    challenge: Uuid,
    specification_sha256: String,
    observation: Value,
}
fn conflict() -> ApiError {
    ApiError::Conflict("original physical environment controller changed".into())
}
fn hash(value: &impl serde::Serialize) -> Result<String, ApiError> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}
pub(super) async fn current(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    instance: Uuid,
    backend: &str,
) -> Result<Option<Value>, ApiError> {
    Ok(sqlx::query_scalar("SELECT to_jsonb(c)-'receipt' FROM management_vm_controllers c JOIN management_instances i ON i.executor_host_id=c.host_id JOIN management_executor_hosts h ON h.id=c.host_id WHERE c.organisation_id=$1 AND i.organisation_id=$1 AND i.id=$2 AND c.backend=$3 AND c.enabled AND h.enabled AND h.trust_domain=$1 AND c.specification->>'instance_id'=$2::text AND c.executor_host_binding=jsonb_build_object('version',1,'host_id',h.id,'trust_domain',h.trust_domain,'daemon_id',h.daemon_id,'operator_state_id',h.operator_state_id) FOR SHARE OF c,h").bind(org).bind(instance).bind(backend).fetch_optional(&mut **tx).await?)
}
pub(super) fn fingerprint(profile: &Value, controller: &Value) -> Result<String, ApiError> {
    if controller.is_null() {
        Ok(profile["specification_sha256"]
            .as_str()
            .ok_or_else(conflict)?
            .into())
    } else {
        hash(
            &json!({"profile":profile["specification_sha256"],"controller_id":controller["id"],"controller_revision":controller["revision"],"controller_sha256":controller["specification_sha256"]}),
        )
    }
}
fn spec(value: &Value, instance: &Value, binding: &Value) -> Result<(), ApiError> {
    if value.as_object().is_none_or(|o| {
        o.len() != 13
            || o.keys().any(|k| {
                !matches!(
                    k.as_str(),
                    "format"
                        | "backend"
                        | "instance_id"
                        | "organisation_id"
                        | "host_id"
                        | "operator_state_id"
                        | "controller_id"
                        | "vm_id"
                        | "guest_hardware_uuid"
                        | "configuration_sha256"
                        | "guest_daemon_id"
                        | "controller_identity"
                        | "limits"
                )
            })
    }) {
        return Err(conflict());
    }
    if value["format"] != 1
        || value["instance_id"] != instance["id"]
        || value["organisation_id"] != instance["organisation_id"]
        || value["host_id"] != binding["host_id"]
        || value["operator_state_id"] != binding["operator_state_id"]
        || !matches!(
            value["backend"].as_str(),
            Some("libvirt" | "kubevirt" | "hyper_v" | "attached" | "open_shell")
        )
    {
        return Err(conflict());
    }
    for key in ["controller_id", "vm_id", "guest_hardware_uuid"] {
        if value[key]
            .as_str()
            .and_then(|v| Uuid::parse_str(v).ok())
            .is_none_or(|v| v.is_nil())
        {
            return Err(conflict());
        }
    }
    if value["configuration_sha256"].as_str().is_none_or(|v| {
        v.len() != 64
            || !v
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) || value["guest_daemon_id"]
        .as_str()
        .is_none_or(|s| s.len() < 12 || s.len() > 128)
        || value["controller_identity"]
            .as_str()
            .is_none_or(|s| s.is_empty() || s.len() > 256)
    {
        return Err(conflict());
    }
    let policy: PolicyRevision = serde_json::from_value(instance["policy"].clone())?;
    for (key, max) in [
        ("cpu_millis", policy.limits.cpu_millis),
        ("memory_mib", policy.limits.memory_mib),
        ("storage_gib", policy.limits.storage_gib),
    ] {
        if value["limits"][key]
            .as_u64()
            .is_none_or(|v| v == 0 || v > u64::from(max))
        {
            return Err(conflict());
        }
    }
    Ok(())
}
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    instance_access(&state, &principal, org, instance, true).await?;
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(c)-'receipt' FROM management_vm_controllers c WHERE organisation_id=$1 AND specification->>'instance_id'=$2::text ORDER BY backend").bind(org).bind(instance).fetch_all(&state.pool).await?))
}
pub(super) async fn prepare(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance)): Path<(Uuid, Uuid)>,
    Json(request): Json<Request>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    if request.request_id.is_nil()
        || request.expected_controller_revision < 0
        || request.confirmation != "ENROLL_EXACT_ENVIRONMENT_CONTROLLER"
    {
        return Err(conflict());
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    sso::authorize(&mut tx, org, &principal).await?;
    let digest = hash(&request)?;
    if let Some(row)=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(p) FROM management_vm_controller_plans p WHERE organisation_id=$1 AND actor_user_id=$2 AND request_id=$3").bind(org).bind(principal.user_id).bind(request.request_id).fetch_optional(&mut *tx).await?{if row["request_sha256"]!=digest{return Err(conflict());}return Ok(Json(row));}
    let instance_row:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 AND organisation_id=$2 AND state='ready' FOR UPDATE").bind(instance).bind(org).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?;
    if instance_row["revision"] != request.expected_instance_revision {
        return Err(conflict());
    }
    environments::instance_unlocked(&mut tx, instance).await?;
    vm_snapshots::unlocked(&mut tx, instance).await?;
    let profile = backend_profiles::current(&mut tx, org, instance)
        .await?
        .ok_or_else(conflict)?;
    if profile["specification"]["kind"] != "vm_compose" {
        return Err(invalid(
            "enroll the original positive VM parent profile first",
        ));
    }
    let binding = &profile["executor_host_binding"];
    spec(&request.specification, &instance_row, binding)?;
    if (request.specification["backend"] == "open_shell")
        != (request.specification["vm_id"] == profile["specification"]["vm_id"])
    {
        return Err(invalid("OpenShell requires the original product VM; independent VM controllers require a different owned VM"));
    }
    let occupied:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(c) FROM management_vm_controllers c WHERE host_id=$1 AND backend=$2 FOR UPDATE").bind(serde_json::from_value::<Uuid>(binding["host_id"].clone())?).bind(request.specification["backend"].as_str()).fetch_optional(&mut *tx).await?;
    if occupied.as_ref().is_some_and(|c| {
        c["organisation_id"] != json!(org) || c["specification"]["instance_id"] != json!(instance)
    }) {
        return Err(conflict());
    }
    environments::admission(&mut tx, &instance_row, None).await?;
    let old = current(
        &mut tx,
        org,
        instance,
        request.specification["backend"]
            .as_str()
            .ok_or_else(conflict)?,
    )
    .await?;
    if old
        .as_ref()
        .and_then(|v| v["revision"].as_i64())
        .unwrap_or(0)
        != request.expected_controller_revision
    {
        return Err(conflict());
    }
    let actor = job_authority::capture(&state, &mut tx, &principal, org).await?;
    let id = Uuid::new_v4();
    let challenge = Uuid::new_v4();
    let sha = hash(&request.specification)?;
    let authority = json!({"login":actor,"instance_revision":instance_row["revision"],"backend_profile":profile});
    sqlx::query("INSERT INTO management_vm_controller_plans(id,organisation_id,host_id,actor_user_id,request_id,request_sha256,actor_authority,expected_revision,specification,specification_sha256,executor_host_binding,challenge) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)").bind(id).bind(org).bind(serde_json::from_value::<Uuid>(binding["host_id"].clone())?).bind(principal.user_id).bind(request.request_id).bind(digest).bind(authority).bind(request.expected_controller_revision).bind(request.specification).bind(&sha).bind(binding).bind(challenge).execute(&mut *tx).await?;
    let row: Value =
        sqlx::query_scalar("SELECT to_jsonb(p) FROM management_vm_controller_plans p WHERE id=$1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "environment.controller.plan",
        id,
        json!({"specification_sha256":sha,"challenge":challenge}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(row))
}
pub(super) async fn activate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Activation>,
) -> Result<Json<Value>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    let row:Value=sqlx::query_scalar("SELECT to_jsonb(p)||jsonb_build_object('live',expires_at>now()) FROM management_vm_controller_plans p WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    operator.verify_operation(&row["executor_host_binding"])?;
    if row["challenge"] != json!(request.challenge)
        || row["specification_sha256"] != request.specification_sha256
    {
        return Err(conflict());
    }
    if !row["receipt"].is_null() {
        if row["receipt"] != request.observation {
            return Err(conflict());
        }
        return Ok(Json(row["receipt"].clone()));
    }
    if row["live"] != true {
        return Err(conflict());
    }
    let org: Uuid = serde_json::from_value(row["organisation_id"].clone())?;
    let instance: Uuid = serde_json::from_value(row["specification"]["instance_id"].clone())?;
    let principal = Principal {
        user_id: serde_json::from_value(row["actor_user_id"].clone())?,
        session_id: None,
        bootstrap: false,
    };
    sso::authorize(&mut tx, org, &principal).await?;
    job_authority::validate(&state, &mut tx, &row["actor_authority"]["login"], org).await?;
    environments::instance_unlocked(&mut tx, instance).await?;
    let instance_row:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 AND organisation_id=$2 AND state='ready' FOR UPDATE").bind(instance).bind(org).fetch_one(&mut *tx).await?;
    if instance_row["revision"] != row["actor_authority"]["instance_revision"]
        || backend_profiles::current(&mut tx, org, instance)
            .await?
            .as_ref()
            != Some(&row["actor_authority"]["backend_profile"])
    {
        return Err(conflict());
    }
    let spec = &row["specification"];
    let occupied:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(c) FROM management_vm_controllers c WHERE host_id=$1 AND backend=$2 FOR UPDATE").bind(serde_json::from_value::<Uuid>(row["host_id"].clone())?).bind(spec["backend"].as_str()).fetch_optional(&mut *tx).await?;
    if occupied.as_ref().is_some_and(|c| {
        c["organisation_id"] != json!(org) || c["specification"]["instance_id"] != json!(instance)
    }) {
        return Err(conflict());
    }
    let observed = &request.observation;
    if observed["format"] != "cowork-physical-environment-controller-v1"
        || observed["plan_id"] != json!(id)
        || observed["challenge"] != json!(request.challenge)
        || observed["configuration_sha256"] != spec["configuration_sha256"]
        || observed["actual_hypervisor_verified"] != true
        || observed["actual_guest_identity_verified"] != true
        || observed["physical_limits_verified"] != true
    {
        return Err(conflict());
    }
    for key in [
        "controller_id",
        "backend",
        "vm_id",
        "guest_daemon_id",
        "guest_hardware_uuid",
        "controller_identity",
        "limits",
    ] {
        if observed[key] != spec[key] {
            return Err(conflict());
        }
    }
    let old = current(
        &mut tx,
        org,
        instance,
        spec["backend"].as_str().ok_or_else(conflict)?,
    )
    .await?;
    let revision = old
        .as_ref()
        .and_then(|v| v["revision"].as_i64())
        .unwrap_or(0);
    if row["expected_revision"] != revision {
        return Err(conflict());
    }
    let cid: Uuid = serde_json::from_value(spec["controller_id"].clone())?;
    if old.as_ref().is_some_and(|v| v["id"] != json!(cid)) {
        return Err(conflict());
    }
    sqlx::query("INSERT INTO management_vm_controllers(id,organisation_id,host_id,backend,revision,plan_id,specification,specification_sha256,executor_host_binding,receipt) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(host_id,backend) DO UPDATE SET revision=EXCLUDED.revision,plan_id=EXCLUDED.plan_id,specification=EXCLUDED.specification,specification_sha256=EXCLUDED.specification_sha256,executor_host_binding=EXCLUDED.executor_host_binding,receipt=EXCLUDED.receipt,enabled=true").bind(old.as_ref().map_or(cid,|v|serde_json::from_value(v["id"].clone()).unwrap_or(cid))).bind(org).bind(serde_json::from_value::<Uuid>(row["host_id"].clone())?).bind(spec["backend"].as_str()).bind(revision+1).bind(id).bind(spec).bind(&request.specification_sha256).bind(&row["executor_host_binding"]).bind(observed).execute(&mut *tx).await?;
    sqlx::query("UPDATE management_vm_controller_plans SET receipt=$2 WHERE id=$1")
        .bind(id)
        .bind(observed)
        .execute(&mut *tx)
        .await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "environment.controller.activate",
        cid,
        json!({"plan_id":id,"receipt_sha256":hash(observed)?}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(request.observation))
}
pub(super) async fn authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Activation>,
) -> Result<Json<Value>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    let row:Value=sqlx::query_scalar("SELECT to_jsonb(p)||jsonb_build_object('live',expires_at>now()) FROM management_vm_controller_plans p WHERE id=$1 FOR SHARE").bind(id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    operator.verify_operation(&row["executor_host_binding"])?;
    if row["live"] != true
        || !row["receipt"].is_null()
        || row["challenge"] != json!(request.challenge)
        || row["specification_sha256"] != request.specification_sha256
    {
        return Err(conflict());
    }
    let org: Uuid = serde_json::from_value(row["organisation_id"].clone())?;
    let instance: Uuid = serde_json::from_value(row["specification"]["instance_id"].clone())?;
    let principal = Principal {
        user_id: serde_json::from_value(row["actor_user_id"].clone())?,
        session_id: None,
        bootstrap: false,
    };
    sso::authorize(&mut tx, org, &principal).await?;
    job_authority::validate(&state, &mut tx, &row["actor_authority"]["login"], org).await?;
    let current:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 AND organisation_id=$2 AND state='ready' FOR SHARE").bind(instance).bind(org).fetch_one(&mut *tx).await?;
    if current["revision"] != row["actor_authority"]["instance_revision"]
        || backend_profiles::current(&mut tx, org, instance)
            .await?
            .as_ref()
            != Some(&row["actor_authority"]["backend_profile"])
    {
        return Err(conflict());
    }
    environments::instance_unlocked(&mut tx, instance).await?;
    vm_snapshots::unlocked(&mut tx, instance).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"plan_id":id,"specification_sha256":request.specification_sha256,"authorized":true}),
    ))
}
#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Power {
    request_id: Uuid,
    expected_instance_revision: i64,
    expected_controller_revision: i64,
    action: String,
    confirmation_vm_id: Uuid,
}
pub(super) async fn power(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance, controller)): Path<(Uuid, Uuid, Uuid)>,
    Json(request): Json<Power>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    if request.request_id.is_nil()
        || !matches!(request.action.as_str(), "power_start" | "power_stop")
    {
        return Err(conflict());
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    sso::authorize(&mut tx, org, &principal).await?;
    let digest = hash(&json!({"controller":controller,"instance":instance,"request":request}))?;
    if let Some(prior)=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(j)-'plan'-'lease_token' FROM management_environment_jobs j WHERE organisation_id=$1 AND actor_user_id=$2 AND request_id=$3").bind(org).bind(principal.user_id).bind(request.request_id).fetch_optional(&mut *tx).await?{if prior["request_sha256"]!=digest{return Err(conflict());}return Ok(Json(prior));}
    environments::instance_unlocked(&mut tx, instance).await?;
    vm_snapshots::unlocked(&mut tx, instance).await?;
    let i:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 AND organisation_id=$2 AND state='ready' FOR UPDATE").bind(instance).bind(org).fetch_one(&mut *tx).await?;
    if i["revision"] != request.expected_instance_revision {
        return Err(conflict());
    }
    let c:Value=sqlx::query_scalar("SELECT to_jsonb(c)-'receipt' FROM management_vm_controllers c WHERE id=$1 AND organisation_id=$2").bind(controller).bind(org).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    if c["backend"] == "open_shell"
        || c["revision"] != request.expected_controller_revision
        || c["specification"]["vm_id"] != json!(request.confirmation_vm_id)
        || current(
            &mut tx,
            org,
            instance,
            c["backend"].as_str().ok_or_else(conflict)?,
        )
        .await?
        .as_ref()
            != Some(&c)
    {
        return Err(conflict());
    }
    let profile = backend_profiles::current(&mut tx, org, instance)
        .await?
        .ok_or_else(conflict)?;
    if c["specification"]["vm_id"] == profile["specification"]["vm_id"] {
        return Err(invalid(
            "product API/control VM cannot be powered through an environment controller",
        ));
    }
    let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_operations WHERE instance_id=$1 AND state IN ('queued','running','reconciling')) OR EXISTS(SELECT 1 FROM management_execution_reservations WHERE instance_id=$1 AND state='held')").bind(instance).fetch_one(&mut *tx).await?;
    if busy || rollouts::reserved(&mut tx, instance).await? {
        return Err(conflict());
    }
    let actor = job_authority::capture(&state, &mut tx, &principal, org).await?;
    let id = Uuid::new_v4();
    let plan = json!({"format":1,"id":id,"organisation_id":org,"instance_id":instance,"actor_user_id":principal.user_id,"actor_authority":actor,"action":request.action,"challenge":Uuid::new_v4(),"instance":i,"backend_profile":profile,"executor_host_binding":profile["executor_host_binding"],"environment":null,"template":null,"environment_controller":c});
    let sha = hash(&plan)?;
    sqlx::query("INSERT INTO management_environment_jobs(id,organisation_id,instance_id,actor_user_id,request_id,request_sha256,action,plan,plan_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(id).bind(org).bind(instance).bind(principal.user_id).bind(request.request_id).bind(digest).bind(&request.action).bind(plan).bind(&sha).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "environment.vm.power.plan",
        id,
        json!({"controller":controller,"action":request.action,"plan_sha256":sha}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":"queued","plan_sha256":sha})))
}
