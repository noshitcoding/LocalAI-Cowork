//! Explicit initial releases do not inherit mutable operator defaults or update authority.
use super::*;

static ENABLED: OnceLock<bool> = OnceLock::new();

pub(super) fn initialize() -> anyhow::Result<()> {
    ENABLED
        .set(
            std::env::var("COWORK_MANAGEMENT_RELEASE_SELECTION_ENABLED")
                .unwrap_or_else(|_| "false".into())
                .parse::<bool>()?,
        )
        .map_err(|_| anyhow::anyhow!("release selection already initialized"))
}

pub(super) fn enabled() -> bool {
    *ENABLED.get().unwrap_or(&false)
}

pub(super) async fn provisioning_releases(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<Vec<ProvisioningRelease>>, ApiError> {
    if membership(&state, &principal, org, false).await? == "auditor" {
        return Err(denied());
    }
    if !enabled() {
        return Ok(Json(vec![]));
    }
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'name',manifest->>'name') FROM management_releases WHERE active AND refreshed_at>now()-interval '5 minutes' ORDER BY manifest->>'name',id")
        .fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()?,
    ))
}

async fn allowed(
    tx: &mut Transaction<'_, Postgres>,
    release: &str,
    host: Option<Uuid>,
) -> Result<bool, ApiError> {
    if !enabled() || !releases::digest(release) {
        return Ok(false);
    }
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_releases r WHERE id=$1 AND (($2::uuid IS NULL AND r.active AND r.refreshed_at>now()-interval '5 minutes') OR EXISTS(SELECT 1 FROM management_executor_host_releases hr JOIN management_executor_hosts h ON h.id=hr.host_id WHERE hr.host_id=$2 AND hr.release_id=r.id AND hr.active AND h.enabled AND hr.refreshed_at>now()-interval '5 minutes')))")
        .bind(release).bind(host).fetch_one(&mut **tx).await?)
}

pub(super) async fn host_releases(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, host)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<ProvisioningRelease>>, ApiError> {
    membership(&state, &principal, org, true).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM management_executor_hosts WHERE id=$1 AND trust_domain=$2)",
    )
    .bind(host)
    .bind(org)
    .fetch_one(&state.pool)
    .await?;
    if !exists {
        return Err(denied());
    }
    if !enabled() {
        return Ok(Json(vec![]));
    }
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',r.id,'name',r.manifest->>'name') FROM management_releases r JOIN management_executor_host_releases hr ON hr.release_id=r.id JOIN management_executor_hosts h ON h.id=hr.host_id WHERE hr.host_id=$1 AND h.trust_domain=$2 AND h.enabled AND hr.active AND hr.refreshed_at>now()-interval '5 minutes' ORDER BY r.manifest->>'name',r.id")
        .bind(host).bind(org).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()?,
    ))
}

pub(super) async fn validate_selection(
    tx: &mut Transaction<'_, Postgres>,
    release: Option<&str>,
    backend: DeploymentBackend,
    policy: &PolicyRevision,
    host: Option<Uuid>,
) -> Result<(), ApiError> {
    if let Some(release) = release {
        let supported = backend == DeploymentBackend::Compose
            && (policy.minimum_isolation == Isolation::Container
                || policy.minimum_isolation == Isolation::VirtualMachine && host.is_some())
            || backend == DeploymentBackend::Kubernetes
                && policy.minimum_isolation == Isolation::Container
                && host.is_some();
        if !supported || !allowed(tx, release, host).await? {
            return Err(invalid("initial release is unavailable for this profile"));
        }
    }
    Ok(())
}

pub(super) async fn arguments(
    tx: &mut Transaction<'_, Postgres>,
    instance: &Instance,
    arguments: &mut Value,
) -> Result<(), ApiError> {
    if arguments["action"] != "provision" {
        return Ok(());
    }
    let host = executor_hosts::instance_host(tx, instance.id).await?;
    validate_selection(
        tx,
        instance.provisioning_release.as_deref(),
        instance.backend,
        &instance.policy,
        host,
    )
    .await?;
    if let Some(release) = &instance.provisioning_release {
        arguments["release"] = json!(release);
        arguments["provisioning_release_version"] = json!(1);
    }
    Ok(())
}

fn selection(operation: &Value, instance: &Value) -> Result<Option<String>, ApiError> {
    if operation["action"] != "provision" {
        return Ok(None);
    }
    let selected = instance["provisioning_release"].as_str();
    if operation["arguments"]["release"].as_str() != selected
        || (selected.is_some()
            && operation["arguments"]["provisioning_release_version"] != json!(1))
        || (selected.is_none() && !operation["arguments"]["provisioning_release_version"].is_null())
    {
        return Err(invalid("initial release binding mismatch"));
    }
    Ok(selected.map(str::to_owned))
}

pub(super) fn operator_contract(
    operation: &Value,
    instance: &Value,
    version: Option<u32>,
) -> Result<(), ApiError> {
    if selection(operation, instance)?.is_some() && version != Some(1) {
        return Err(invalid(
            "operator must support provisioning release version 1",
        ));
    }
    Ok(())
}

pub(super) async fn dispatch_allowed(
    tx: &mut Transaction<'_, Postgres>,
    operation: &Value,
    instance: &Value,
) -> Result<bool, ApiError> {
    let Some(release) = selection(operation, instance)? else {
        return Ok(true);
    };
    let host = serde_json::from_value(instance["executor_host_id"].clone())?;
    let profiled = !operation["backend_profile"].is_null();
    Ok(((instance["backend"] == "compose"
        && instance["policy"]["minimum_isolation"] == "container")
        || profiled)
        && allowed(tx, &release, host).await?)
}

pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    instance_id: Uuid,
    arguments: &Value,
    result: &Value,
) -> Result<(), ApiError> {
    let instance: Value =
        sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 FOR UPDATE")
            .bind(instance_id)
            .fetch_one(&mut **tx)
            .await?;
    let selected = selection(
        &json!({"action":"provision","arguments":arguments}),
        &instance,
    )?;
    if let Some(release) = selected {
        if result["receipt_id"] != json!(id)
            || result["instance_id"] != json!(instance_id)
            || result["release"] != release
        {
            return Err(invalid(
                "provisioning receipt does not match its initial release",
            ));
        }
        // A completed, bound receipt may arrive after catalog withdrawal. It proves
        // existing work; it never authorizes another provisioning attempt.
        sqlx::query("UPDATE management_instances SET current_release=$2 WHERE id=$1")
            .bind(instance_id)
            .bind(release)
            .execute(&mut **tx)
            .await?;
    } else if !result["release"].is_null() {
        return Err(invalid("unexpected provisioning release receipt"));
    }
    Ok(())
}
