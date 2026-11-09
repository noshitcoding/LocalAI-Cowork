//! Operator-measured VM/Kubernetes profiles, original assignments and CAS.
use super::*;

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Prepare {
    request_id: Uuid,
    expected_instance_revision: i64,
    expected_profile_revision: i64,
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
fn mismatch() -> ApiError {
    ApiError::Conflict("backend original plan, physical profile or assignment changed".into())
}
pub(super) async fn current(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    instance: Uuid,
) -> Result<Option<Value>, ApiError> {
    Ok(sqlx::query_scalar("SELECT to_jsonb(p)-'receipt' FROM management_backend_profiles p JOIN management_instances i ON i.id=p.instance_id JOIN management_executor_hosts h ON h.id=p.host_id WHERE p.instance_id=$1 AND p.organisation_id=$2 AND p.enabled AND h.enabled AND h.trust_domain=$2 AND i.executor_host_id=p.host_id AND p.executor_host_binding=jsonb_build_object('version',1,'host_id',h.id,'trust_domain',h.trust_domain,'daemon_id',h.daemon_id,'operator_state_id',h.operator_state_id) FOR SHARE OF p,h")
        .bind(instance).bind(org).fetch_optional(&mut **tx).await?)
}
fn specification(value: &Value, instance: &Value, host: &Value) -> Result<(), ApiError> {
    let kind = value["kind"].as_str().ok_or_else(mismatch)?;
    if value["format"] != 1
        || value["instance_id"] != instance["id"]
        || value["organisation_id"] != instance["organisation_id"]
        || value["host_id"] != instance["executor_host_id"]
        || value["operator_state_id"] != host["operator_state_id"]
    {
        return Err(mismatch());
    }
    let id: Uuid = serde_json::from_value(instance["id"].clone())?;
    if value["public_origin"]
        != crate::instance_gateway::origin(id)?
            .as_str()
            .trim_end_matches('/')
    {
        return Err(mismatch());
    }
    if kind == "vm_compose" {
        if instance["backend"] != "compose"
            || instance["policy"]["minimum_isolation"] != "virtual_machine"
            || value["daemon_id"] != host["daemon_id"]
            || value["vm_id"]
                .as_str()
                .and_then(|v| Uuid::parse_str(v).ok())
                .is_none()
            || value["routing"]["target_kind"] != "owned_vm_api_forward"
        {
            return Err(mismatch());
        }
    } else if kind == "kubernetes" {
        if instance["backend"] != "kubernetes"
            || instance["policy"]["minimum_isolation"] != "container"
            || value["cluster_uid"] != host["daemon_id"]
            || value["namespace"] != format!("cowork-{}", id.simple())
            || value["namespace_uid"]
                .as_str()
                .and_then(|v| Uuid::parse_str(v).ok())
                .is_none()
            || value["runner"]["backend"] != "kubernetes"
            || value["runner"]["instance_id"] != instance["id"]
            || value["runner"]["organisation_id"] != instance["organisation_id"]
            || value["routing"]["target_kind"] != "owned_namespace_api_service"
        {
            return Err(mismatch());
        }
    } else {
        return Err(mismatch());
    }
    for digest in [
        value["operator_configuration_sha256"].as_str(),
        value["routing"]["configuration_sha256"].as_str(),
    ] {
        if digest.is_none_or(|d| {
            d.len() != 64
                || !d
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        }) {
            return Err(mismatch());
        }
    }
    Ok(())
}
pub(super) async fn inspect(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    let mut tx = state.pool.begin().await?;
    let profile = current(&mut tx, org, id).await?;
    tx.commit().await?;
    Ok(Json(json!({"profile":profile})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HostIdentity {
    executor_host_binding: executor_hosts::HostBinding,
}
pub(super) async fn runtime_identity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((org, instance)): Path<(Uuid, Uuid)>,
    Json(request): Json<HostIdentity>,
) -> Result<Json<Value>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    let profile = current(&mut tx, org, instance)
        .await?
        .ok_or_else(mismatch)?;
    operator.verify_operation(&profile["executor_host_binding"])?;
    if profile["specification"]["kind"] != "vm_compose" {
        return Err(mismatch());
    }
    let i:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 AND organisation_id=$2 AND state<>'deleted' FOR SHARE").bind(instance).bind(org).fetch_one(&mut *tx).await?;
    let receipt:Value=sqlx::query_scalar("SELECT receipt FROM management_backend_profiles WHERE instance_id=$1 AND organisation_id=$2 AND revision=$3 AND enabled FOR SHARE").bind(instance).bind(org).bind(profile["revision"].as_i64().ok_or_else(mismatch)?).fetch_one(&mut *tx).await?;
    let b = &profile["executor_host_binding"];
    let value = json!({"format":"cowork-manager-host-runtime-identity-v1","organisation_id":org,"instance_id":instance,"execution_epoch":i["execution_epoch"],"instance_revision":i["revision"],"host_id":b["host_id"],"operator_state_id":b["operator_state_id"],"daemon_id":b["daemon_id"],"vm_id":profile["specification"]["vm_id"],"executor_host_binding":b,"backend_profile_revision":profile["revision"],"specification_sha256":profile["specification_sha256"],"backend_activation_receipt_sha256":hex::encode(Sha256::digest(serde_json::to_vec(&receipt)?)),"execution_required":i["execution_required"]});
    tx.commit().await?;
    Ok(Json(value))
}
pub(super) async fn prepare(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
    Json(request): Json<Prepare>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    if request.confirmation != "PREPARE_EXACT_BACKEND_PROFILE"
        || request.request_id.is_nil()
        || request.expected_profile_revision < 0
    {
        return Err(invalid("confirm the original backend profile plan"));
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    sso::authorize(&mut tx, org, &principal).await?;
    let instance:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 AND organisation_id=$2 FOR UPDATE")
        .bind(id).bind(org).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    if !matches!(
        instance["state"].as_str(),
        Some("pending" | "stopped" | "failed")
    ) {
        return Err(mismatch());
    }
    let request_sha = hex::encode(Sha256::digest(serde_json::to_vec(&request)?));
    let prior:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM management_backend_profile_plans p WHERE organisation_id=$1 AND actor_user_id=$2 AND request_id=$3")
        .bind(org).bind(principal.user_id).bind(request.request_id).fetch_optional(&mut *tx).await?;
    if let Some(prior) = prior {
        if prior["request_sha256"] != request_sha {
            return Err(mismatch());
        }
        return Ok(Json(prior));
    }
    if instance["revision"].as_i64() != Some(request.expected_instance_revision) {
        return Err(mismatch());
    }
    environments::instance_unlocked(&mut tx, id).await?;
    vm_snapshots::unlocked(&mut tx, id).await?;
    let host:Value=sqlx::query_scalar("SELECT to_jsonb(h) FROM management_executor_hosts h WHERE id=$1 AND trust_domain=$2 AND enabled FOR SHARE")
        .bind(serde_json::from_value::<Uuid>(instance["executor_host_id"].clone())?).bind(org).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    specification(&request.specification, &instance, &host)?;
    let current_revision: Option<i64> = sqlx::query_scalar(
        "SELECT revision FROM management_backend_profiles WHERE instance_id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if current_revision.unwrap_or(0) != request.expected_profile_revision {
        return Err(mismatch());
    }
    let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_operations WHERE instance_id=$1 AND state IN ('queued','running','reconciling'))").bind(id).fetch_one(&mut *tx).await?;
    if busy || rollouts::reserved(&mut tx, id).await? {
        return Err(mismatch());
    }
    let binding = json!({"version":1,"host_id":host["id"],"trust_domain":org,"daemon_id":host["daemon_id"],"operator_state_id":host["operator_state_id"]});
    let plan = Uuid::new_v4();
    let challenge = Uuid::new_v4();
    let specification_sha =
        hex::encode(Sha256::digest(serde_json::to_vec(&request.specification)?));
    let result:Value=sqlx::query_scalar("INSERT INTO management_backend_profile_plans(id,organisation_id,instance_id,host_id,actor_user_id,request_id,request_sha256,expected_instance_revision,expected_profile_revision,specification,specification_sha256,executor_host_binding,challenge) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) RETURNING to_jsonb(management_backend_profile_plans)")
        .bind(plan).bind(org).bind(id).bind(serde_json::from_value::<Uuid>(host["id"].clone())?).bind(principal.user_id).bind(request.request_id).bind(request_sha).bind(request.expected_instance_revision).bind(request.expected_profile_revision).bind(request.specification).bind(specification_sha).bind(binding).bind(challenge).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(result))
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
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    let plan: Value = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM management_backend_profile_plans p WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    operator.verify_operation(&plan["executor_host_binding"])?;
    if plan["challenge"] != json!(request.challenge)
        || plan["specification_sha256"] != request.specification_sha256
    {
        return Err(mismatch());
    }
    let observation = &request.observation;
    if observation["format"] != 1
        || observation["challenge"] != plan["challenge"]
        || observation["specification_sha256"] != plan["specification_sha256"]
        || observation["executor_host_binding"] != plan["executor_host_binding"]
        || observation["routing"]["serving_https_verified"] != true
        || observation["routing"]["configuration_sha256"]
            != plan["specification"]["routing"]["configuration_sha256"]
    {
        return Err(mismatch());
    }
    match plan["specification"]["kind"].as_str() {
        Some("vm_compose") => {
            if observation["physical"]["kvm_enabled"] != true
                || observation["physical"]["qemu_owned"] != true
                || observation["physical"]["vm_id"] != plan["specification"]["vm_id"]
                || observation["physical"]["daemon_id"] != plan["specification"]["daemon_id"]
                || observation["physical"]["mutual_tls_verified"] != true
            {
                return Err(mismatch());
            }
        }
        Some("kubernetes") => {
            if observation["physical"]["cluster_uid"] != plan["specification"]["cluster_uid"]
                || observation["physical"]["namespace_uid"]
                    != plan["specification"]["namespace_uid"]
                || observation["physical"]["runner"]["backend"] != "kubernetes"
                || observation["physical"]["runner"]["instance_id"] != plan["instance_id"]
                || observation["physical"]["runner"]["organisation_id"] != plan["organisation_id"]
            {
                return Err(mismatch());
            }
        }
        _ => return Err(mismatch()),
    }
    if !plan["receipt"].is_null() {
        if plan["receipt"]["observation"] != *observation {
            return Err(mismatch());
        }
        return Ok(Json(plan["receipt"].clone()));
    }
    let fresh: bool = sqlx::query_scalar(
        "SELECT expires_at>clock_timestamp() FROM management_backend_profile_plans WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if !fresh {
        return Err(ApiError::Conflict(
            "backend plan expired; prepare a fresh original plan".into(),
        ));
    }
    let org: Uuid = serde_json::from_value(plan["organisation_id"].clone())?;
    let instance_id: Uuid = serde_json::from_value(plan["instance_id"].clone())?;
    sqlx::query(
        "SELECT id FROM management_organisations WHERE id=$1 AND archived_at IS NULL FOR UPDATE",
    )
    .bind(org)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    let authority:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u LEFT JOIN management_members m ON m.user_id=u.id AND m.organisation_id=$1 WHERE u.id=$2 AND u.deleted_at IS NULL AND (u.platform_admin OR m.role='admin'))")
        .bind(org).bind(serde_json::from_value::<Uuid>(plan["actor_user_id"].clone())?).fetch_one(&mut *tx).await?;
    if !authority {
        return Err(denied());
    }
    let instance:Value=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 AND organisation_id=$2 FOR UPDATE").bind(instance_id).bind(org).fetch_one(&mut *tx).await?;
    if instance["revision"] != plan["expected_instance_revision"]
        || instance["executor_host_id"] != plan["host_id"]
    {
        return Err(mismatch());
    }
    let previous: Option<i64> = sqlx::query_scalar(
        "SELECT revision FROM management_backend_profiles WHERE instance_id=$1 FOR UPDATE",
    )
    .bind(instance_id)
    .fetch_optional(&mut *tx)
    .await?;
    if previous.unwrap_or(0)
        != plan["expected_profile_revision"]
            .as_i64()
            .ok_or_else(mismatch)?
    {
        return Err(mismatch());
    }
    let revision = previous.unwrap_or(0) + 1;
    let receipt = json!({"format":"cowork-backend-activation-v1","plan_id":id,"instance_id":instance_id,"organisation_id":org,"profile_revision":revision,"specification_sha256":request.specification_sha256,"observation":observation});
    sqlx::query("INSERT INTO management_backend_profiles(instance_id,organisation_id,host_id,revision,plan_id,specification,specification_sha256,executor_host_binding,receipt) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(instance_id) DO UPDATE SET host_id=excluded.host_id,revision=excluded.revision,plan_id=excluded.plan_id,specification=excluded.specification,specification_sha256=excluded.specification_sha256,executor_host_binding=excluded.executor_host_binding,receipt=excluded.receipt,enabled=TRUE")
        .bind(instance_id).bind(org).bind(operator.host_id().ok_or_else(denied)?).bind(revision).bind(id).bind(&plan["specification"]).bind(&request.specification_sha256).bind(&plan["executor_host_binding"]).bind(&receipt).execute(&mut *tx).await?;
    sqlx::query("UPDATE management_backend_profile_plans SET receipt=$2 WHERE id=$1")
        .bind(id)
        .bind(&receipt)
        .execute(&mut *tx)
        .await?;
    append_audit(&mut tx,org,serde_json::from_value(plan["actor_user_id"].clone())?,"instance.backend.activate",instance_id,json!({"plan_id":id,"profile_revision":revision,"specification_sha256":request.specification_sha256})).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
pub(super) fn available(
    instance: &Instance,
    action: InstanceAction,
    profile: Option<&Value>,
) -> bool {
    if !PROVISIONING_ENABLED.get().copied().unwrap_or(true) {
        return false;
    }
    let Some(p) = profile else {
        return false;
    };
    let kind = p["specification"]["kind"].as_str();
    if !(kind == Some("vm_compose")
        && instance.backend == DeploymentBackend::Compose
        && instance.policy.minimum_isolation == Isolation::VirtualMachine
        || kind == Some("kubernetes")
            && instance.backend == DeploymentBackend::Kubernetes
            && instance.policy.minimum_isolation == Isolation::Container)
    {
        return false;
    }
    match action {
        InstanceAction::Delete => deletion::enabled(),
        InstanceAction::Backup | InstanceAction::Restore => {
            BACKUPS_ENABLED.get().copied().unwrap_or(false)
                && instance.state == InstanceState::Stopped
        }
        InstanceAction::Upgrade | InstanceAction::Rollback => {
            UPDATES_ENABLED.get().copied().unwrap_or(false)
                && BACKUPS_ENABLED.get().copied().unwrap_or(false)
                && instance.state == InstanceState::Stopped
                && instance.current_release.is_some()
        }
        InstanceAction::EnrollExecution => {
            execution_enrollment::enabled()
                && BACKUPS_ENABLED.get().copied().unwrap_or(false)
                && instance.state == InstanceState::Stopped
        }
        _ => PROVISIONING_ENABLED.get().copied().unwrap_or(true),
    }
}
pub(super) async fn frozen(
    tx: &mut Transaction<'_, Postgres>,
    operation: &Value,
    version: Option<u32>,
) -> Result<Option<Value>, ApiError> {
    let frozen = &operation["backend_profile"];
    if frozen.is_null() {
        if operation["instance"]["backend"] == "kubernetes"
            || operation["instance"]["policy"]["minimum_isolation"] == "virtual_machine"
        {
            return Err(ApiError::Unavailable(
                "an activated original backend profile is required".into(),
            ));
        }
        return Ok(None);
    }
    if version != Some(1) {
        return Err(ApiError::Unavailable(
            "backend profile operator contract version 1 is required".into(),
        ));
    }
    let org: Uuid = serde_json::from_value(operation["organisation_id"].clone())?;
    let id: Uuid = serde_json::from_value(operation["instance_id"].clone())?;
    let actual = current(tx, org, id).await?.ok_or_else(mismatch)?;
    if actual != *frozen {
        return Err(mismatch());
    }
    Ok(Some(actual))
}
