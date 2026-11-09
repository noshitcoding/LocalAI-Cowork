//! Fleet management. Enabled only in the dedicated management process mode.
//! All customer content remains in separately provisioned application databases.
use crate::{
    auth::{self, Principal},
    db,
    error::ApiError,
    AppState,
};
use axum::{
    extract::{Extension, Path, State},
    http::HeaderMap,
    middleware,
    routing::{delete, get, post, put},
    Json, Router,
};
use cowork_contracts::enterprise::*;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use std::sync::OnceLock;
use uuid::Uuid;
mod audit_lifecycle;
mod backend_profiles;
mod disk_quotas;
mod environment_access;
mod environment_execution;
mod environments;
mod executor_hosts;
mod host_capacity;
mod job_authority;
mod provisioning_releases;
mod releases;
mod vm_controllers;
mod vm_snapshots;
use provisioning_releases::provisioning_releases;
mod deletion;
mod execution_admission;
mod execution_coverage;
mod execution_enrollment;
mod execution_generation;
mod execution_recovery;
use execution_admission::{inspect as execution_capacity, update as set_execution_budget};
pub(crate) mod content;
pub(crate) mod content_federation;
pub(crate) mod directory;
pub(crate) mod invitations;
pub(crate) mod mfa;
use mfa::{inspect as mfa_policy, update as update_mfa_policy};
pub(crate) mod organisation_lifecycle;
mod resource_catalog;
mod rollbacks;
mod rollouts;
pub(crate) mod sso;
pub(crate) mod wallet;
use invitations::{accept_invitation, create_invitation, list_invitations, revoke_invitation};
use sso::{sso_settings, update_sso};
mod instance_imports;
mod instance_members;
use instance_members::{
    instance_member_candidates, instance_members, remove_instance_member, set_instance_member,
};
use releases::{instance_releases, sync_releases};
use rollbacks::instance_rollbacks;
use rollouts::{control_rollout, create_rollout, list_rollouts};

static PLATFORM_CEILING: OnceLock<PolicyRevision> = OnceLock::new();
static PROVISIONING_ENABLED: OnceLock<bool> = OnceLock::new();
static BACKUPS_ENABLED: OnceLock<bool> = OnceLock::new();
static UPDATES_ENABLED: OnceLock<bool> = OnceLock::new();

pub fn initialize() -> anyhow::Result<()> {
    provisioning_releases::initialize()?;
    deletion::initialize()?;
    execution_enrollment::initialize()?;
    execution_admission::initialize()?;
    audit_key()?;
    UPDATES_ENABLED
        .set(
            std::env::var("COWORK_MANAGEMENT_UPDATES_ENABLED")
                .unwrap_or_else(|_| "false".into())
                .parse::<bool>()?,
        )
        .map_err(|_| anyhow::anyhow!("management already initialized"))?;
    BACKUPS_ENABLED
        .set(
            std::env::var("COWORK_MANAGEMENT_BACKUPS_ENABLED")
                .unwrap_or_else(|_| "false".into())
                .parse::<bool>()?,
        )
        .map_err(|_| anyhow::anyhow!("management already initialized"))?;
    PROVISIONING_ENABLED
        .set(
            std::env::var("COWORK_MANAGEMENT_PROVISIONING_ENABLED")
                .unwrap_or_else(|_| "true".into())
                .parse::<bool>()?,
        )
        .map_err(|_| anyhow::anyhow!("management already initialized"))?;
    let ceiling: PolicyRevision = match std::env::var("COWORK_MANAGEMENT_POLICY_CEILING_FILE") {
        Ok(path) => serde_json::from_slice(&std::fs::read(path)?)?,
        Err(std::env::VarError::NotPresent) => PolicyRevision::default(),
        Err(error) => return Err(error.into()),
    };
    ceiling.validate().map_err(anyhow::Error::msg)?;
    PLATFORM_CEILING
        .set(ceiling)
        .map_err(|_| anyhow::anyhow!("management already initialized"))?;
    Ok(())
}

fn platform_ceiling() -> &'static PolicyRevision {
    PLATFORM_CEILING
        .get()
        .expect("management is initialized before serving requests")
}

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .merge(content_federation::router())
        .merge(directory::management_router())
        .route("/organisations/{org}/wallets", get(wallet::list))
        .route(
            "/organisations/{org}/wallet-settings",
            put(wallet::settings),
        )
        .route(
            "/organisations/{org}/members/{user}/wallet",
            get(wallet::inspect).put(wallet::update),
        )
        .route(
            "/organisations/{org}/members/{user}/wallet/entries",
            post(wallet::entry),
        )
        .route(
            "/organisations/{org}/members/{user}/wallet/reservations/{reservation}/resolve",
            post(wallet::resolve),
        )
        .route(
            "/resource-catalog",
            get(resource_catalog::list).post(resource_catalog::create),
        )
        .route(
            "/resource-catalog/{resource}",
            put(resource_catalog::update).delete(resource_catalog::remove),
        )
        .route(
            "/organisations/{org}/members/{user}/resource-assignments",
            get(resource_catalog::assignments).put(resource_catalog::set_assignments),
        )
        .route(
            "/organisations/{org}/resources",
            get(resource_catalog::visible),
        )
        .route("/capabilities", get(organisation_lifecycle::capabilities))
        .route(
            "/organisations/{org}/settings",
            get(organisation_lifecycle::inspect).put(organisation_lifecycle::rename),
        )
        .route(
            "/organisations/{org}/archive",
            post(organisation_lifecycle::archive),
        )
        .route(
            "/organisations/{org}/mfa-policy",
            get(mfa_policy).put(update_mfa_policy),
        )
        .route(
            "/organisations/{org}/instance-imports",
            post(instance_imports::prepare),
        )
        .route(
            "/organisations/{org}/instance-imports/{import}",
            get(instance_imports::inspect),
        )
        .route(
            "/organisations/{org}/instance-imports/{import}/cutover",
            post(instance_imports::cutover),
        )
        .route(
            "/organisations/{org}/instance-imports/{import}/abort",
            post(instance_imports::abort),
        )
        .route(
            "/organisations/{org}/instance-routes/{instance}",
            get(instance_imports::route),
        )
        .route(
            "/organisations/{org}/executor-hosts",
            get(executor_hosts::list).post(executor_hosts::register),
        )
        .route(
            "/organisations/{org}/executor-hosts/{host}",
            put(executor_hosts::control),
        )
        .route(
            "/organisations/{org}/executor-hosts/{host}/credentials",
            post(executor_hosts::issue_credential),
        )
        .route(
            "/organisations/{org}/executor-hosts/{host}/credentials/{credential}",
            delete(executor_hosts::revoke_credential),
        )
        .route(
            "/organisations/{org}/executor-hosts/{host}/instances",
            get(executor_hosts::instances).post(executor_hosts::create_instance),
        )
        .route(
            "/organisations/{org}/executor-hosts/{host}/provisioning-releases",
            get(provisioning_releases::host_releases),
        )
        .route(
            "/organisations",
            get(organisations).post(create_organisation),
        )
        .route("/organisations/{org}/policy", put(update_policy))
        .route(
            "/organisations/{org}/provisioning-releases",
            get(provisioning_releases),
        )
        .route(
            "/organisations/{org}/execution-capacity",
            get(execution_capacity).put(set_execution_budget),
        )
        .route(
            "/organisations/{org}/sso",
            get(sso_settings).put(update_sso),
        )
        .route(
            "/organisations/{org}/invitations",
            get(list_invitations).post(create_invitation),
        )
        .route(
            "/organisations/{org}/invitations/{invitation}",
            delete(revoke_invitation),
        )
        .route("/invitations/accept", post(accept_invitation))
        .route(
            "/organisations/{org}/rollouts",
            get(list_rollouts).post(create_rollout),
        )
        .route(
            "/organisations/{org}/rollouts/{rollout}/control",
            post(control_rollout),
        )
        .route(
            "/organisations/{org}/members",
            get(members).post(set_member),
        )
        .route(
            "/organisations/{org}/instances",
            get(instances).post(create_instance),
        )
        .route(
            "/organisations/{org}/instances/{instance}/operations",
            get(operations).post(request_operation),
        )
        .route(
            "/organisations/{org}/instances/{instance}/backups",
            get(backups),
        )
        .route(
            "/organisations/{org}/instances/{instance}/releases",
            get(instance_releases),
        )
        .route(
            "/organisations/{org}/instances/{instance}/rollbacks",
            get(instance_rollbacks),
        )
        .route(
            "/organisations/{org}/instances/{instance}/operations/{operation}/recover",
            post(request_recovery),
        )
        .route(
            "/organisations/{org}/instances/{instance}/launch",
            post(request_launch),
        )
        .route(
            "/organisations/{org}/instances/{instance}/members",
            get(instance_members).post(set_instance_member),
        )
        .route(
            "/organisations/{org}/instances/{instance}/members/candidates",
            get(instance_member_candidates),
        )
        .route(
            "/organisations/{org}/instances/{instance}/members/{user}",
            delete(remove_instance_member),
        )
        .route(
            "/organisations/{org}/instances/{instance}/environments",
            get(environments).post(environments::create),
        )
        .route(
            "/organisations/{org}/instances/{instance}/environments/{environment}/access",
            get(environment_access::inspect).put(environment_access::update),
        )
        .route(
            "/organisations/{org}/instances/{instance}/vm-snapshots",
            get(vm_snapshots::list).post(vm_snapshots::prepare),
        )
        .route(
            "/organisations/{org}/instances/{instance}/environment-controllers",
            get(vm_controllers::list).post(vm_controllers::prepare),
        )
        .route(
            "/organisations/{org}/instances/{instance}/environment-controllers/{controller}/power",
            post(vm_controllers::power),
        )
        .route(
            "/organisations/{org}/instances/{instance}/vm-snapshot-jobs",
            get(vm_snapshots::jobs),
        )
        .route(
            "/organisations/{org}/templates",
            get(templates).post(create_template),
        )
        .route(
            "/organisations/{org}/instances/{instance}/templates/{template}/qualify",
            post(environments::qualify),
        )
        .route(
            "/organisations/{org}/instances/{instance}/environments/{environment}/actions",
            post(environments::action),
        )
        .route(
            "/organisations/{org}/instances/{instance}/environment-jobs/{job}",
            get(environments::inspect_job),
        )
        .route(
            "/organisations/{org}/instances/{instance}/environment-jobs",
            get(environments::jobs),
        )
        .route(
            "/organisations/{org}/instances/{instance}/environment-jobs/{job}/recover",
            post(environments::recover),
        )
        .route("/organisations/{org}/audit", get(audit_log))
        .route(
            "/organisations/{org}/instances/{instance}/backend-profile",
            get(backend_profiles::inspect).post(backend_profiles::prepare),
        )
        .route(
            "/organisations/{org}/audit/checkpoint",
            get(audit_lifecycle::checkpoint),
        )
        .route(
            "/organisations/{org}/audit/exports",
            post(audit_lifecycle::prepare),
        )
        .route(
            "/organisations/{org}/audit/exports/{export}",
            get(audit_lifecycle::inspect),
        )
        .route(
            "/organisations/{org}/audit/exports/{export}/complete",
            post(audit_lifecycle::complete),
        )
        .merge(disk_quotas::router())
        .route_layer(middleware::from_fn_with_state(state, auth::require_auth))
}

pub async fn register_execution_admission(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    execution_admission::register(pool).await
}
pub async fn execution_ready(pool: &sqlx::PgPool) -> Result<(), ApiError> {
    execution_admission::ready(pool).await
}

pub fn provisioner_router() -> Router<AppState> {
    Router::new()
        .route(
            "/resource-models/{org}/{instance}/{user}/{model}/v1/chat/completions",
            post(resource_catalog::model_proxy::complete),
        )
        .route(
            "/resource-entitlements",
            get(resource_catalog::entitlements),
        )
        .merge(content_federation::runner_router())
        .merge(disk_quotas::operator_router())
        .route("/instance-access", post(crate::fleet::inspect_access))
        .route(
            "/host-runtime-identity/{org}/{instance}",
            post(backend_profiles::runtime_identity),
        )
        .route("/execution-reserve", post(execution_admission::reserve))
        .route("/execution-release", post(execution_admission::release))
        .route("/content-access", post(content::inspect))
        .route("/mfa-authority", post(mfa::authority))
        .route("/content-capability", post(content::announce))
        .route("/claim", post(claim_operation))
        .route(
            "/environment-execution",
            post(environment_execution::submit),
        )
        .route(
            "/environment-execution/destinations",
            get(environment_execution::destinations),
        )
        .route(
            "/environment-controllers/{plan}/activate",
            post(vm_controllers::activate),
        )
        .route(
            "/environment-controllers/{plan}/authorize",
            post(vm_controllers::authorize),
        )
        .route(
            "/environment-execution/{request}",
            get(environment_execution::inspect),
        )
        .route(
            "/environment-execution/{request}/stop",
            post(environment_execution::stop),
        )
        .route(
            "/environment-execution/claim",
            post(environment_execution::claim),
        )
        .route(
            "/environment-execution/pending",
            post(environment_execution::pending),
        )
        .route(
            "/environment-execution/{job}/authorize",
            post(environment_execution::authorize),
        )
        .route(
            "/environment-execution/{job}/complete",
            post(environment_execution::complete),
        )
        .route(
            "/environment-execution/{job}/hold",
            post(environment_execution::hold),
        )
        .route("/environments/claim", post(environments::claim))
        .route("/environments/pending", post(environments::pending))
        .route("/environments/{job}/complete", post(environments::complete))
        .route("/environments/{job}/hold", post(environments::hold))
        .route(
            "/environments/{job}/authorize",
            post(environments::authorize),
        )
        .route("/vm-snapshots/claim", post(vm_snapshots::claim))
        .route("/vm-snapshots/pending", post(vm_snapshots::pending))
        .route(
            "/vm-snapshots/{job}/authorize",
            post(vm_snapshots::authorize),
        )
        .route("/vm-snapshots/{job}/hold", post(vm_snapshots::hold))
        .route("/vm-snapshots/{job}/complete", post(vm_snapshots::complete))
        .route(
            "/backend-profiles/{plan}/activate",
            post(backend_profiles::activate),
        )
        .route("/host-capacity/challenge", post(host_capacity::challenge))
        .route("/host-capacity/attest", post(host_capacity::attest))
        .route("/pending", post(pending_operations))
        .route("/releases/sync", post(sync_releases))
        .route("/operations/{operation}/complete", post(complete_operation))
        .route(
            "/operations/{operation}/reconciliation",
            post(report_reconciliation),
        )
        .layer(middleware::map_response(
            |mut response: axum::response::Response| async move {
                response.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
                response
            },
        ))
}

fn provisioner_auth(headers: &HeaderMap) -> Result<(), ApiError> {
    use subtle::ConstantTimeEq;
    let path = std::env::var("COWORK_PROVISIONER_TOKEN_FILE")
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("provisioner token is not configured")))?;
    let key = std::fs::read_to_string(path).map_err(|e| ApiError::Internal(e.into()))?;
    let given = headers
        .get("x-cowork-provisioner-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if key.trim().len() < 32
        || !bool::from(
            Sha256::digest(key.trim().as_bytes()).ct_eq(&Sha256::digest(given.as_bytes())),
        )
    {
        return Err(denied());
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimRequest {
    backend: DeploymentBackend,
    backend_profile_version: Option<u32>,
    execution_admission_version: Option<u32>,
    execution_enrollment_version: Option<u32>,
    provisioning_release_version: Option<u32>,
    executor_host_binding: Option<executor_hosts::HostBinding>,
}

async fn pending_operations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ClaimRequest>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(request.executor_host_binding.as_ref())?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    let enabled = operator.current(&mut tx).await?;
    sqlx::query("UPDATE management_operations SET state='reconciling',updated_at=now() WHERE state='running' AND lease_expires_at<now()")
        .execute(&mut *tx).await?;
    // Read-only recovery dispatch: retains the original lease and never makes
    // an in-flight operation eligible for claim again.
    let mut pending: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(o)||jsonb_build_object('instance',to_jsonb(i)) FROM management_operations o JOIN management_instances i ON i.id=o.instance_id WHERE o.state IN ('running','reconciling') AND i.backend=$1 AND i.executor_host_id IS NOT DISTINCT FROM $2 ORDER BY o.created_at FOR UPDATE OF o")
        .bind(tag(request.backend)?).bind(operator.host_id()).fetch_all(&mut *tx).await?;
    for operation in &mut pending {
        backend_profiles::frozen(&mut tx, operation, request.backend_profile_version).await?;
        mfa::attach(&mut tx, &mut operation["instance"]).await?;
        operator.verify_operation(&operation["executor_host_binding"])?;
        host_capacity::attach_frozen_operation(&mut tx, operation).await?;
        provisioning_releases::operator_contract(
            operation,
            &operation["instance"],
            request.provisioning_release_version,
        )?;
        execution_coverage::operator_contract(
            operation["execution_runtime_required"] == true,
            request.execution_admission_version,
        )?;
        execution_enrollment::operator_contract(
            &operation["action"],
            request.execution_enrollment_version,
        )?;
        executor_hosts::attach_operator_secrets(&mut tx, &operator, operation).await?;
        operation["recovery_authorized"] = json!(false);
        if operation["state"] != "reconciling" || operation["recovery_requested_by"].is_null() {
            continue;
        }
        let actor: Uuid = serde_json::from_value(operation["recovery_requested_by"].clone())?;
        let org: Uuid = serde_json::from_value(operation["organisation_id"].clone())?;
        let instance: Uuid = serde_json::from_value(operation["instance_id"].clone())?;
        let id: Uuid = serde_json::from_value(operation["id"].clone())?;
        let owner: Uuid = serde_json::from_value(operation["instance"]["owner_user_id"].clone())?;
        let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u LEFT JOIN management_members m ON m.user_id=u.id AND m.organisation_id=$2 WHERE u.id=$1 AND u.deleted_at IS NULL AND (u.platform_admin OR m.role='admin' OR (m.role='member' AND NOT $5 AND ($3=u.id OR EXISTS(SELECT 1 FROM management_instance_members im WHERE im.instance_id=$4 AND im.user_id=u.id AND im.role='manager')))))")
            .bind(actor).bind(org).bind(owner).bind(instance)
            .bind(matches!(operation["action"].as_str(), Some("suspend" | "resume" | "enroll_execution")) || execution_recovery::requested(&operation["arguments"])).fetch_one(&mut *tx).await?;
        let dispatch = enabled
            && allowed
            && (operation["action"] != "enroll_execution" || execution_enrollment::enabled())
            && (!matches!(
                operation["action"].as_str(),
                Some("delete" | "enroll_execution")
            ) || operation["recovery_dispatched_at"].is_null());
        operation["recovery_authorized"] = json!(dispatch);
        if dispatch && operation["recovery_dispatched_at"].is_null() {
            sqlx::query(
                "UPDATE management_operations SET recovery_dispatched_at=now() WHERE id=$1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
            append_audit(
                &mut tx,
                org,
                actor,
                "instance.operation.recovery_dispatched",
                instance,
                json!({"operation_id":id}),
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok(Json(pending))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReconciliationReport {
    lease_token: Uuid,
    reason: String,
}

async fn report_reconciliation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<ReconciliationReport>,
) -> Result<Json<Value>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    if ![
        "operation_completion_unproven",
        "operation_recovery_requires_review",
    ]
    .contains(&request.reason.as_str())
    {
        return Err(invalid("invalid reconciliation reason"));
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    operator.current(&mut tx).await?;
    let row = sqlx::query(
        "SELECT * FROM management_operations WHERE id=$1 AND lease_token=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(request.lease_token)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    operator.verify_operation(
        &row.try_get::<Option<Value>, _>("executor_host_binding")?
            .unwrap_or(Value::Null),
    )?;
    let previous: String = row.try_get("state")?;
    if previous == "succeeded" || previous == "failed" {
        return Ok(Json(json!({"state":previous})));
    }
    let result = json!({"error_code":request.reason});
    if previous != "reconciling"
        || row.try_get::<Option<Value>, _>("result")? != Some(result.clone())
    {
        sqlx::query("UPDATE management_operations SET state='reconciling',result=$2,updated_at=now() WHERE id=$1").bind(id).bind(result).execute(&mut *tx).await?;
        append_audit(
            &mut tx,
            row.try_get("organisation_id")?,
            row.try_get("actor_user_id")?,
            "instance.operation.reconciliation_required",
            row.try_get("instance_id")?,
            json!({"operation_id":id,"reason":request.reason}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"state":"reconciling"})))
}

async fn claim_operation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ClaimRequest>,
) -> Result<Json<Value>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(request.executor_host_binding.as_ref())?;
    if !operator.enabled {
        return Ok(Json(Value::Null));
    }
    rollouts::advance(&state).await?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    if !operator.current(&mut tx).await? {
        return Ok(Json(Value::Null));
    }
    // Timed-out operations need reconciliation, never a second unsafe execution.
    sqlx::query("UPDATE management_operations SET state='reconciling',updated_at=now() WHERE state='running' AND lease_expires_at<now()")
        .execute(&mut *tx).await?;
    let op: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(o) FROM management_operations o JOIN management_instances i ON i.id=o.instance_id WHERE o.state='queued' AND i.backend=$1 AND i.executor_host_id IS NOT DISTINCT FROM $2 AND (o.rollout_id IS NULL OR EXISTS(SELECT 1 FROM management_rollouts r WHERE r.id=o.rollout_id AND r.state='running')) ORDER BY o.created_at FOR UPDATE OF o SKIP LOCKED LIMIT 1")
        .bind(tag(request.backend)?).bind(operator.host_id()).fetch_optional(&mut *tx).await?;
    let Some(mut operation) = op else {
        tx.commit().await?;
        return Ok(Json(Value::Null));
    };
    operator.verify_operation(&operation["executor_host_binding"])?;
    let op_id: Uuid = serde_json::from_value(operation["id"].clone())?;
    let instance_id: Uuid = serde_json::from_value(operation["instance_id"].clone())?;
    let org: Uuid = serde_json::from_value(operation["organisation_id"].clone())?;
    if instance_imports::requested(&operation["action"]) {
        execution_admission::lock_global(&mut tx).await?;
    }
    execution_generation::lock_organisation(&mut tx, org).await?;
    let token = Uuid::new_v4();
    let mut instance: Value =
        sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 FOR UPDATE")
            .bind(instance_id)
            .fetch_one(&mut *tx)
            .await?;
    mfa::attach(&mut tx, &mut instance).await?;
    operation["instance"] = instance.clone();
    let backend_profile =
        backend_profiles::frozen(&mut tx, &operation, request.backend_profile_version).await?;
    instance_imports::dispatch(&mut tx, &mut operation).await?;
    execution_coverage::operator_contract(
        instance["execution_required"] == true
            || execution_enrollment::requested(&operation["action"]),
        request.execution_admission_version,
    )?;
    execution_enrollment::operator_contract(
        &operation["action"],
        request.execution_enrollment_version,
    )?;
    provisioning_releases::operator_contract(
        &operation,
        &instance,
        request.provisioning_release_version,
    )?;
    let release_version =
        if operation["action"] == "provision" && !instance["provisioning_release"].is_null() {
            Some(1_i32)
        } else {
            None
        };
    sqlx::query("UPDATE management_operations SET state='running',lease_token=$2,lease_expires_at=now()+interval '1 hour',provisioning_release_version=$3,updated_at=now() WHERE id=$1")
        .bind(op_id).bind(token).bind(release_version).execute(&mut *tx).await?;
    let actor: Uuid = serde_json::from_value(operation["actor_user_id"].clone())?;
    // Recheck authority at dispatch: demotion or account removal while queued
    // must not leave an executable administrative capability behind.
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u LEFT JOIN management_members m ON m.user_id=u.id AND m.organisation_id=$2 WHERE u.id=$1 AND u.deleted_at IS NULL AND (u.platform_admin OR m.role='admin' OR (m.role='member' AND NOT $5 AND ($3=u.id OR EXISTS(SELECT 1 FROM management_instance_members im WHERE im.instance_id=$4 AND im.user_id=u.id AND im.role='manager')))))")
        .bind(actor).bind(org).bind(serde_json::from_value::<Uuid>(instance["owner_user_id"].clone())?).bind(instance_id)
        .bind(matches!(operation["action"].as_str(), Some("suspend" | "resume" | "enroll_execution")) || instance_imports::requested(&operation["action"]) || execution_recovery::requested(&operation["arguments"])).fetch_one(&mut *tx).await?;
    let update_invalid = if operation["action"] == "upgrade" {
        let current: Instance = serde_json::from_value(instance.clone())?;
        !(operation_available(&current, InstanceAction::Upgrade)
            || backend_profiles::available(
                &current,
                InstanceAction::Upgrade,
                backend_profile.as_ref(),
            ))
            || operation["arguments"]["source_release"] != instance["current_release"]
            || !releases::selection_allowed_profiled(
                &mut tx,
                operation["arguments"]["release"].as_str().unwrap_or(""),
                operation["arguments"]["source_release"]
                    .as_str()
                    .unwrap_or(""),
                operation["arguments"]["qualification_only"]
                    .as_bool()
                    .unwrap_or(false),
                operator.host_id(),
                backend_profile.as_ref(),
            )
            .await?
    } else if operation["action"] == "rollback" {
        let current: Instance = serde_json::from_value(instance.clone())?;
        !(operation_available(&current, InstanceAction::Rollback)
            || backend_profiles::available(
                &current,
                InstanceAction::Rollback,
                backend_profile.as_ref(),
            ))
            || !rollbacks::validate(&mut tx, org, instance_id, &operation["arguments"]).await?
    } else if instance["execution_required"] == true
        && matches!(operation["action"].as_str(), Some("start" | "resume"))
        && !execution_admission::active()
    {
        true
    } else if operation["action"] == "enroll_execution" {
        !(execution_enrollment::valid_instance(&instance)
            || backend_profile.is_some()
                && instance["execution_required"] == false
                && instance["state"] == "stopped")
    } else if operation["action"] == "delete" {
        !deletion::enabled()
    } else if execution_recovery::requested(&operation["arguments"]) {
        !execution_admission::active()
            || !matches!(instance["state"].as_str(), Some("ready" | "stopped"))
    } else {
        false
    };
    let rollout_allowed = rollouts::dispatch_allowed(&mut tx, &operation).await?;
    let provision_allowed =
        provisioning_releases::dispatch_allowed(&mut tx, &operation, &instance).await?;
    if !allowed || update_invalid || !rollout_allowed || !provision_allowed {
        let reason = if !allowed || !rollout_allowed {
            "request_authority_revoked"
        } else {
            "release_authorization_revoked"
        };
        sqlx::query("UPDATE management_operations SET state='failed',result=$2,updated_at=now() WHERE id=$1").bind(op_id).bind(json!({"error_code":reason})).execute(&mut *tx).await?;
        append_audit(
            &mut tx,
            org,
            actor,
            "instance.operation.rejected",
            instance_id,
            json!({"operation_id":op_id,"reason":reason}),
        )
        .await?;
        tx.commit().await?;
        return Ok(Json(Value::Null));
    }
    let org_policy: Value =
        sqlx::query_scalar("SELECT policy FROM management_organisations WHERE id=$1")
            .bind(org)
            .fetch_one(&mut *tx)
            .await?;
    let policy = serde_json::from_value::<PolicyRevision>(instance["policy"].clone())?
        .restricted_by(&serde_json::from_value(org_policy)?)
        .restricted_by(platform_ceiling());
    if instance_imports::requested(&operation["action"])
        && instance["policy"] != serde_json::to_value(&policy)?
    {
        return Err(ApiError::Conflict(
            "import requires its unchanged effective source and target policies".into(),
        ));
    }
    instance["policy"] = serde_json::to_value(&policy)?;
    sqlx::query("UPDATE management_instances SET policy=$2 WHERE id=$1")
        .bind(instance_id)
        .bind(&instance["policy"])
        .execute(&mut *tx)
        .await?;
    if operation["action"] == "provision" {
        sqlx::query("UPDATE management_instances SET state='provisioning' WHERE id=$1")
            .bind(instance_id)
            .execute(&mut *tx)
            .await?;
    }
    if operation["action"] == "delete" {
        sqlx::query("UPDATE management_instances SET state='deleting' WHERE id=$1")
            .bind(instance_id)
            .execute(&mut *tx)
            .await?;
    }
    operation["lease_token"] = json!(token);
    if operation["action"] == "restore" || operation["action"] == "rollback" {
        let backup_id: Uuid = serde_json::from_value(operation["arguments"]["backup_id"].clone())?;
        operation["backup"] = sqlx::query_scalar::<_, Value>("SELECT to_jsonb(b) FROM management_backups b WHERE id=$1 AND organisation_id=$2 AND instance_id=$3")
            .bind(backup_id).bind(org).bind(instance_id).fetch_one(&mut *tx).await?;
    }
    execution_generation::prepare(&mut tx, &mut operation, &instance).await?;
    execution_recovery::prepare(&mut tx, &mut operation).await?;
    host_capacity::prepare_operation(&mut tx, &mut operation, &instance).await?;
    operation["instance"] = instance;
    executor_hosts::attach_operator_secrets(&mut tx, &operator, &mut operation).await?;
    tx.commit().await?;
    Ok(Json(operation))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteOperation {
    lease_token: Uuid,
    succeeded: bool,
    result: Value,
}
async fn complete_operation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<CompleteOperation>,
) -> Result<Json<Value>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    if serde_json::to_vec(&request.result)?.len() > 16384 {
        return Err(invalid("operation result is too large"));
    }
    // Result contains only receipt identifiers and non-sensitive status, never command output.
    if !request.result.is_object()
        || request.result.as_object().is_some_and(|v| {
            v.keys().any(|k| {
                ![
                    "receipt_id",
                    "backup_id",
                    "release",
                    "error_code",
                    "instance_state",
                    "backup_sha256",
                    "backup_bytes",
                    "manifest_version",
                    "instance_id",
                    "source_release",
                    "qualification_only",
                    "rollback_verified",
                    "generation",
                    "rollback_operation_id",
                    "restored_backup_id",
                    "deletion_verified",
                    "deletion_inventory_sha256",
                    "deleted_backup_count",
                    "execution",
                    "execution_recovery",
                    "execution_enrollment",
                    "executor_host_binding",
                    "host_allocation",
                    "instance_import",
                    "backend_profile",
                ]
                .contains(&k.as_str())
            })
        })
    {
        return Err(invalid("invalid operation receipt"));
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    let importing: bool = sqlx::query_scalar(
        "SELECT instance_import_id IS NOT NULL FROM management_operations WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .unwrap_or(false);
    if operator.host_id().is_some() || importing {
        execution_admission::lock_global(&mut tx).await?;
    }
    operator.current(&mut tx).await?;
    let row = sqlx::query(
        "SELECT * FROM management_operations WHERE id=$1 AND lease_token=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(request.lease_token)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    operator.verify_operation(
        &row.try_get::<Option<Value>, _>("executor_host_binding")?
            .unwrap_or(Value::Null),
    )?;
    operator.verify_operation(&request.result["executor_host_binding"])?;
    let previous: String = row.try_get("state")?;
    if previous == "succeeded" || previous == "failed" {
        if row.try_get::<Option<Value>, _>("result")? != Some(request.result.clone())
            || (previous == "succeeded") != request.succeeded
        {
            return Err(ApiError::Conflict(
                "operation already completed with a different receipt".into(),
            ));
        }
        return Ok(Json(json!({"state":previous})));
    }
    host_capacity::validate_operation_receipt(&row, &request.result)?;
    let frozen_profile: Option<Value> = row.try_get("backend_profile")?;
    if let Some(profile) = &frozen_profile {
        if request.result["backend_profile"]["specification_sha256"]
            != profile["specification_sha256"]
            || request.result["backend_profile"]["profile_revision"] != profile["revision"]
            || request.result["backend_profile"]["instance_id"] != profile["instance_id"]
        {
            return Err(invalid("original backend profile receipt binding differs"));
        }
    } else if !request.result["backend_profile"].is_null() {
        return Err(invalid("unexpected backend profile receipt"));
    }
    let org: Uuid = row.try_get("organisation_id")?;
    if row
        .try_get::<Option<Value>, _>("execution_recovery_binding")?
        .is_some()
    {
        execution_admission::lock_global(&mut tx).await?;
    }
    execution_generation::lock_organisation(&mut tx, org).await?;
    let instance: Uuid = row.try_get("instance_id")?;
    let actor: Uuid = row.try_get("actor_user_id")?;
    let action: String = row.try_get("action")?;
    if instance_imports::requested(&json!(action)) && !request.succeeded {
        return Err(invalid(
            "unproven import work requires reconciliation, with both instance locks retained",
        ));
    }
    if action == "delete" {
        if !request.succeeded {
            return Err(invalid("unverified deletion must remain in reconciliation"));
        }
        deletion::complete(
            &mut tx,
            id,
            instance,
            &row.try_get::<Value, _>("arguments")?,
            &request.result,
        )
        .await?;
    }
    if request.succeeded && action == "provision" {
        provisioning_releases::complete(
            &mut tx,
            id,
            instance,
            &row.try_get::<Value, _>("arguments")?,
            &request.result,
        )
        .await?;
    }
    if request.succeeded && action == "upgrade" {
        releases::complete_update(
            &mut tx,
            id,
            instance,
            &row.try_get::<Value, _>("arguments")?,
            &request.result,
        )
        .await?;
    }
    if request.succeeded && action == "rollback" {
        rollbacks::complete(
            &mut tx,
            id,
            instance,
            &row.try_get::<Value, _>("arguments")?,
            &request.result,
        )
        .await?;
    }
    if !request.succeeded
        && matches!(action.as_str(), "upgrade" | "rollback")
        && request.result["instance_state"] != "stopped"
    {
        return Err(invalid(
            "an unverified update must remain in reconciliation",
        ));
    }
    if (request.succeeded && action == "backup")
        || (matches!(action.as_str(), "upgrade" | "rollback" | "enroll_execution")
            && (request.succeeded || !request.result["backup_id"].is_null()))
    {
        let backup_id: Uuid = serde_json::from_value(request.result["backup_id"].clone())
            .map_err(|_| invalid("backup receipt requires a valid backup ID"))?;
        let sha = request.result["backup_sha256"]
            .as_str()
            .ok_or_else(|| invalid("backup digest missing"))?;
        let size = request.result["backup_bytes"]
            .as_i64()
            .filter(|v| *v > 0)
            .ok_or_else(|| invalid("backup size missing"))?;
        if backup_id != id
            || sha.len() != 64
            || !sha
                .bytes()
                .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
            || request.result["manifest_version"] != json!(1)
        {
            return Err(invalid("invalid verified backup receipt"));
        }
        sqlx::query("INSERT INTO management_backups(id,organisation_id,instance_id,operation_id,sha256,size_bytes,manifest_version) VALUES($1,$2,$3,$1,$4,$5,1)")
            .bind(backup_id).bind(org).bind(instance).bind(sha).bind(size).execute(&mut *tx).await?;
    }
    if request.succeeded && action == "restore" {
        let arguments: Value = row.try_get("arguments")?;
        if request.result["backup_id"] != arguments["backup_id"] {
            return Err(invalid(
                "restore receipt does not match the requested backup",
            ));
        }
    }
    let recovered = !request.succeeded
        && request.result["error_code"] == "operation_interrupted_resources_stopped";
    if recovered
        && (previous != "reconciling"
            || row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("recovery_dispatched_at")?
                .is_none()
            || !matches!(
                request.result["instance_state"].as_str(),
                Some("stopped" | "suspended")
            ))
    {
        return Err(invalid("stopping recovery was not dispatched"));
    }
    let new_state = if recovered {
        Some(
            if action == "suspend" || request.result["instance_state"] == "suspended" {
                "suspended"
            } else {
                "stopped"
            },
        )
    } else if !request.succeeded
        && matches!(
            action.as_str(),
            "restore" | "upgrade" | "rollback" | "enroll_execution"
        )
        && request.result["instance_state"] == "stopped"
    {
        Some("stopped") // Trusted operator verified that the original resources are still stopped and active.
    } else if !request.succeeded && action == "backup" {
        None // A failed read-only backup must not change a stopped instance.
    } else if !request.succeeded {
        Some("failed")
    } else {
        match action.as_str() {
            "provision" | "start" | "resume" => Some("ready"),
            "stop" => Some("stopped"),
            "suspend" => Some("suspended"),
            "delete" => Some("deleted"),
            "restore" | "upgrade" | "rollback" | "enroll_execution" => Some("stopped"),
            _ => None,
        }
    };
    execution_recovery::complete(&mut tx, &row, request.succeeded, &request.result).await?;
    execution_generation::complete(&mut tx, &row, request.succeeded, recovered, &request.result)
        .await?;
    execution_coverage::complete(&mut tx, &row, request.succeeded, &request.result).await?;
    execution_enrollment::complete(&mut tx, &row, request.succeeded, &request.result).await?;
    host_capacity::complete(&mut tx, &row, request.succeeded, &request.result).await?;
    if let Some(new_state) = new_state {
        sqlx::query("UPDATE management_instances SET state=$2,revision=revision+1 WHERE id=$1")
            .bind(instance)
            .bind(new_state)
            .execute(&mut *tx)
            .await?;
    }
    let status = if request.succeeded {
        "succeeded"
    } else {
        "failed"
    };
    sqlx::query("UPDATE management_operations SET state=$2,result=$3,updated_at=now() WHERE id=$1")
        .bind(id)
        .bind(status)
        .bind(&request.result)
        .execute(&mut *tx)
        .await?;
    instance_imports::complete(&mut tx, &row, request.succeeded, &request.result).await?;
    append_audit(
        &mut tx,
        org,
        actor,
        "instance.operation.complete",
        instance,
        json!({"operation_id":id,"state":status,"deletion":if action=="delete" {Some(request.result.clone())}else{None}}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"state":status})))
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Unprocessable(message.into())
}
fn denied() -> ApiError {
    ApiError::NotFound("organisation or resource is not accessible".into())
}
fn name(value: &str) -> Result<&str, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 120 || value.chars().any(char::is_control) {
        return Err(invalid("name must contain 1 to 120 printable characters"));
    }
    Ok(value)
}
fn tag<T: serde::Serialize>(value: T) -> Result<String, ApiError> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("invalid enum"))
}
async fn platform(state: &AppState, principal: &Principal) -> Result<bool, ApiError> {
    if principal.bootstrap {
        return Err(denied());
    }
    db::user_is_platform_admin(&state.pool, principal.user_id).await
}
async fn membership(
    state: &AppState,
    principal: &Principal,
    org: Uuid,
    admin: bool,
) -> Result<String, ApiError> {
    if principal.bootstrap {
        return Err(denied());
    }
    // Platform operators can manage infrastructure, but are not implicitly content readers.
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM management_organisations WHERE id=$1 AND archived_at IS NULL)",
    )
    .bind(org)
    .fetch_one(&state.pool)
    .await?;
    if !exists {
        return Err(denied());
    }
    if platform(state, principal).await? {
        return Ok("admin".into());
    }
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM management_members WHERE organisation_id=$1 AND user_id=$2",
    )
    .bind(org)
    .bind(principal.user_id)
    .fetch_optional(&state.pool)
    .await?;
    match role {
        Some(role)
            if matches!(role.as_str(), "admin" | "member" | "auditor")
                && (!admin || role == "admin") =>
        {
            Ok(role)
        }
        _ => Err(denied()),
    }
}
async fn instance_access(
    state: &AppState,
    principal: &Principal,
    org: Uuid,
    id: Uuid,
    write: bool,
) -> Result<Instance, ApiError> {
    let role = membership(state, principal, org, false).await?;
    if write && role == "auditor" {
        return Err(denied());
    }
    let value: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE organisation_id=$1 AND id=$2 AND state <> 'deleted'")
        .bind(org).bind(id).fetch_optional(&state.pool).await?;
    let instance: Instance = serde_json::from_value(value.ok_or_else(denied)?)?;
    if role == "admin" || instance.owner_user_id == principal.user_id {
        return Ok(instance);
    }
    let shared: Option<String> = sqlx::query_scalar("SELECT role FROM management_instance_members WHERE organisation_id=$1 AND instance_id=$2 AND user_id=$3")
        .bind(org).bind(id).bind(principal.user_id).fetch_optional(&state.pool).await?;
    if shared.as_deref().is_some_and(|r| !write || r == "manager") {
        Ok(instance)
    } else {
        Err(denied())
    }
}

pub fn audit_key() -> anyhow::Result<Vec<u8>> {
    let path = std::env::var("COWORK_MANAGEMENT_AUDIT_KEY_FILE")
        .map_err(|_| anyhow::anyhow!("management requires COWORK_MANAGEMENT_AUDIT_KEY_FILE"))?;
    let key = std::fs::read(path)?;
    anyhow::ensure!(
        key.len() >= 32,
        "management audit key must contain at least 32 bytes"
    );
    Ok(key)
}

async fn append_audit(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    actor: Uuid,
    action: &str,
    target: Uuid,
    metadata: Value,
) -> Result<(), ApiError> {
    // Serialize per-organisation heads so concurrent writes cannot fork the chain.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("management-audit:{org}"))
        .execute(&mut **tx)
        .await?;
    let previous: Option<String> = sqlx::query_scalar("SELECT digest FROM management_audit WHERE organisation_id=$1 ORDER BY sequence DESC LIMIT 1").bind(org).fetch_optional(&mut **tx).await?;
    let previous = previous.unwrap_or_else(|| "0".repeat(64));
    let payload = json!({"organisation_id":org,"actor_user_id":actor,"action":action,"target_id":target,"metadata":metadata,"timestamp":chrono::Utc::now()});
    let mut mac = Hmac::<Sha256>::new_from_slice(&audit_key().map_err(ApiError::Internal)?)
        .map_err(|e| ApiError::Internal(e.into()))?;
    mac.update(previous.as_bytes());
    mac.update(&serde_json::to_vec(&payload)?);
    let digest = hex::encode(mac.finalize().into_bytes());
    sqlx::query("INSERT INTO management_audit(organisation_id,actor_user_id,action,target_id,payload,previous_digest,digest) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(org).bind(actor).bind(action).bind(target).bind(payload).bind(previous).bind(digest).execute(&mut **tx).await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Named {
    name: String,
}
async fn organisations(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let admin = platform(&state, &principal).await?;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(o) || jsonb_build_object('role', CASE WHEN $2 THEN 'admin' ELSE m.role END) FROM management_organisations o LEFT JOIN management_members m ON m.organisation_id=o.id AND m.user_id=$1 WHERE o.archived_at IS NULL AND ($2 OR m.role IN ('admin','member','auditor')) ORDER BY o.name")
        .bind(principal.user_id).bind(admin).fetch_all(&state.pool).await?;
    Ok(Json(
        json!({"schema_version":3,"platform_admin":admin,"execution_admission_enabled":execution_admission::active(),"release_selection_enabled":provisioning_releases::enabled(),"instance_imports_enabled":true,"items":rows}),
    ))
}
async fn create_organisation(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<Named>,
) -> Result<Json<Value>, ApiError> {
    if !platform(&state, &principal).await? {
        return Err(denied());
    }
    let id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    let policy = serde_json::to_value(PolicyRevision::default().restricted_by(platform_ceiling()))?;
    sqlx::query("INSERT INTO management_organisations(id,name,policy) VALUES($1,$2,$3)")
        .bind(id)
        .bind(name(&request.name)?)
        .bind(policy)
        .execute(&mut *tx)
        .await?;
    mfa::ensure_policy(&mut tx, id).await?;
    sqlx::query(
        "INSERT INTO management_members(organisation_id,user_id,role) VALUES($1,$2,'admin')",
    )
    .bind(id)
    .bind(principal.user_id)
    .execute(&mut *tx)
    .await?;
    append_audit(
        &mut tx,
        id,
        principal.user_id,
        "organisation.create",
        id,
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id})))
}
async fn update_policy(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(policy): Json<PolicyRevision>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    policy.validate().map_err(invalid)?;
    if !policy.narrows(platform_ceiling()) {
        return Err(invalid(
            "organisation policy exceeds the operator's platform ceiling",
        ));
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    sso::authorize(&mut tx, org, &principal).await?;
    execution_admission::lock_global(&mut tx).await?;
    let row: Value =
        sqlx::query_scalar("SELECT policy FROM management_organisations WHERE id=$1 AND archived_at IS NULL FOR UPDATE")
            .bind(org)
            .fetch_one(&mut *tx)
            .await?;
    let prior: PolicyRevision = serde_json::from_value(row)?;
    if policy.revision != prior.revision + 1 {
        return Err(ApiError::Conflict(
            "reload the current policy before saving".into(),
        ));
    }
    // Until hot policy rollout is qualified, require a quiescent fleet. Never
    // acknowledge tighter policy while already running sandboxes keep old rights.
    let fleet: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE organisation_id=$1 AND state<>'deleted' FOR UPDATE").bind(org).fetch_all(&mut *tx).await?;
    let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_operations WHERE organisation_id=$1 AND state IN ('queued','running','reconciling'))").bind(org).fetch_one(&mut *tx).await?;
    let reserved: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_rollout_targets WHERE organisation_id=$1 AND reserved)").bind(org).fetch_one(&mut *tx).await?;
    if busy
        || reserved
        || fleet
            .iter()
            .any(|v| !["pending", "stopped"].contains(&v["state"].as_str().unwrap_or("")))
    {
        return Err(ApiError::Conflict(
            "stop all organisation instances and finish pending operations and rollouts before changing policy"
                .into(),
        ));
    }
    // Stopped infrastructure does not prove that an offline device has finished.
    // The organisation lock also serialises admission and completion, so a held
    // reservation cannot disappear or be created between this check and commit.
    let executing: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_execution_reservations WHERE organisation_id=$1 AND state='held')")
        .bind(org).fetch_one(&mut *tx).await?;
    if executing {
        return Err(ApiError::Conflict(
            "settle outstanding execution reservations before changing policy".into(),
        ));
    }
    for value in fleet {
        let instance: Instance = serde_json::from_value(value)?;
        let mut effective = instance.policy.restricted_by(&policy);
        effective.revision += 1;
        host_capacity::invalidate_policy(&mut tx, instance.id).await?;
        sqlx::query("UPDATE management_instances SET policy=$2,revision=revision+1 WHERE id=$1")
            .bind(instance.id)
            .bind(serde_json::to_value(effective)?)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE management_organisations SET policy=$2 WHERE id=$1")
        .bind(org)
        .bind(serde_json::to_value(&policy)?)
        .execute(&mut *tx)
        .await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "policy.update",
        org,
        json!({"revision":policy.revision}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(serde_json::to_value(policy)?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberRequest {
    email: String,
    role: OrganisationRole,
    #[serde(default)]
    content_admin: bool,
}
async fn members(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<Vec<Value>>, ApiError> {
    membership(&state, &principal, org, true).await?;
    Ok(Json(sqlx::query_scalar("SELECT jsonb_build_object('user_id',m.user_id,'email',u.email,'display_name',u.display_name,'role',m.role,'content_admin',m.content_admin,'sso_provider_id',m.sso_provider_id) FROM management_members m JOIN users u ON u.id=m.user_id WHERE m.organisation_id=$1 ORDER BY u.email").bind(org).fetch_all(&state.pool).await?))
}
async fn set_member(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(request): Json<MemberRequest>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    if request.content_admin && request.role != OrganisationRole::Admin {
        return Err(invalid(
            "content readers must be organisation administrators",
        ));
    }
    let user: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM users WHERE lower(email)=lower($1) AND deleted_at IS NULL",
    )
    .bind(request.email.trim())
    .fetch_optional(&state.pool)
    .await?;
    let user = user.ok_or_else(|| {
        invalid("invite this account to the management service before assigning membership")
    })?;
    let mut tx = state.pool.begin().await?;
    sso::authorize(&mut tx, org, &principal).await?;
    if request.role != OrganisationRole::Admin {
        if mfa::policy(&mut tx, org).await?.required {
            let recoverable:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_members m JOIN users u ON u.id=m.user_id WHERE m.organisation_id=$1 AND m.user_id<>$2 AND m.role='admin' AND m.sso_provider_id IS NULL AND NOT m.directory_managed AND u.deleted_at IS NULL AND u.password_hash IS NOT NULL AND (EXISTS(SELECT 1 FROM user_totp t WHERE t.user_id=u.id AND t.enabled) OR EXISTS(SELECT 1 FROM passkeys p WHERE p.user_id=u.id)))").bind(org).bind(user).fetch_one(&mut *tx).await?;
            if !recoverable {
                return Err(invalid(
                    "retain a manually assigned administrator with local MFA",
                ));
            }
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM management_members m JOIN users u ON u.id=m.user_id WHERE m.organisation_id=$1 AND m.role='admin' AND m.user_id<>$2 AND u.deleted_at IS NULL AND (m.sso_provider_id IS NULL OR NOT EXISTS(SELECT 1 FROM management_sso WHERE organisation_id=$1))").bind(org).bind(user).fetch_one(&mut *tx).await?;
        if count == 0 {
            return Err(invalid(
                "an organisation must retain at least one administrator",
            ));
        }
    }
    sqlx::query("INSERT INTO management_members(organisation_id,user_id,role,content_admin,directory_managed) VALUES($1,$2,$3,$4,false) ON CONFLICT(organisation_id,user_id) DO UPDATE SET role=EXCLUDED.role,content_admin=EXCLUDED.content_admin,sso_provider_id=NULL,directory_managed=false")
        .bind(org).bind(user).bind(tag(request.role)?).bind(request.content_admin).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "member.update",
        user,
        json!({"role":request.role,"content_admin":request.content_admin}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"user_id":user,"role":request.role})))
}
async fn instances(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let role = membership(&state, &principal, org, false).await?;
    let mut rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(i) || jsonb_build_object('can_manage', $3 OR (owner_user_id=$2 AND $4) OR ($4 AND EXISTS(SELECT 1 FROM management_instance_members m WHERE m.instance_id=i.id AND m.user_id=$2 AND m.role='manager'))) FROM management_instances i WHERE i.organisation_id=$1 AND state<>'deleted' AND ($3 OR owner_user_id=$2 OR EXISTS(SELECT 1 FROM management_instance_members m WHERE m.instance_id=i.id AND m.user_id=$2)) ORDER BY created_at")
        .bind(org).bind(principal.user_id).bind(role == "admin").bind(role != "auditor").fetch_all(&state.pool).await?;
    for value in &mut rows {
        let mut profile_tx = state.pool.begin().await?;
        let profile = backend_profiles::current(
            &mut profile_tx,
            org,
            serde_json::from_value(value["id"].clone())?,
        )
        .await?;
        profile_tx.commit().await?;
        // Internal runtime fencing is not part of the public Instance contract.
        // The authenticated provisioner receives it on its separate claim API.
        if let Some(object) = value.as_object_mut() {
            object.remove("execution_epoch");
            object.remove("execution_evidence");
            object.remove("execution_enrollment");
            object.remove("executor_host_id");
        }
        let instance: Instance = serde_json::from_value(value.clone())?;
        let mut actions: Vec<_> = [
            InstanceAction::Provision,
            InstanceAction::Start,
            InstanceAction::Stop,
            InstanceAction::Suspend,
            InstanceAction::Resume,
            InstanceAction::Backup,
            InstanceAction::Restore,
            InstanceAction::Upgrade,
            InstanceAction::Rollback,
            InstanceAction::EnrollExecution,
            InstanceAction::Delete,
        ]
        .into_iter()
        .filter(|action| {
            instance.state.accepts(*action)
                && (operation_available(&instance, *action)
                    || backend_profiles::available(&instance, *action, profile.as_ref()))
        })
        .collect();
        if role != "admin" || value["execution_required"] != false {
            actions.retain(|a| *a != InstanceAction::EnrollExecution);
        }
        if value["execution_required"] == true && !execution_admission::active() {
            actions.retain(|a| !matches!(a, InstanceAction::Start | InstanceAction::Resume));
        }
        let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_operations WHERE instance_id=$1 AND state IN ('queued','running','reconciling'))").bind(instance.id).fetch_one(&state.pool).await?;
        if !busy && actions.contains(&InstanceAction::Rollback) {
            let mut tx = state.pool.begin().await?;
            if rollbacks::point(&mut tx, org, instance.id).await?.is_none() {
                actions.retain(|a| *a != InstanceAction::Rollback);
            }
            tx.commit().await?;
        }
        let rollout: Option<Uuid> = sqlx::query_scalar(
            "SELECT rollout_id FROM management_rollout_targets WHERE instance_id=$1 AND reserved",
        )
        .bind(instance.id)
        .fetch_optional(&state.pool)
        .await?;
        value["rollout_id"] = json!(rollout);
        value["available_actions"] = if busy || rollout.is_some() {
            json!([])
        } else {
            serde_json::to_value(actions)?
        };
        value["can_open"] = json!(
            (crate::fleet::allowed(
                &state,
                &crate::fleet::AccessRequest {
                    organisation_id: org,
                    instance_id: instance.id,
                    user_id: principal.user_id,
                }
            )
            .await?
                || content::can_launch_viewer(&state, org, instance.id, principal.user_id).await?)
                && std::env::var_os("COWORK_INSTANCE_URL_TEMPLATE").is_some()
        );
        value["can_open_support"] = json!(
            content::can_launch(&state, org, instance.id, principal.user_id).await?
                && std::env::var_os("COWORK_INSTANCE_URL_TEMPLATE").is_some()
        );
    }
    Ok(Json(rows))
}

async fn request_launch(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance)): Path<(Uuid, Uuid)>,
) -> Result<
    (
        [(axum::http::header::HeaderName, &'static str); 1],
        Json<Value>,
    ),
    ApiError,
> {
    let mut tx = state.pool.begin().await?;
    let fleet_key = crate::fleet::current_instance_key(&mut tx, org, instance).await?;
    let member = crate::fleet::allowed_on(
        &mut *tx,
        &crate::fleet::AccessRequest {
            organisation_id: org,
            instance_id: instance,
            user_id: principal.user_id,
        },
    )
    .await?;
    let viewer = content::can_launch_viewer_on(&mut *tx, org, instance, principal.user_id).await?;
    if principal.bootstrap
        || !(member
            || viewer
            || content::can_launch_on(&mut *tx, org, instance, principal.user_id).await?)
    {
        return Err(denied());
    }
    let url = crate::instance_gateway::origin(instance)?;
    let user =
        sqlx::query("SELECT email,display_name FROM users WHERE id=$1 AND deleted_at IS NULL")
            .bind(principal.user_id)
            .fetch_one(&mut *tx)
            .await?;
    let email: String = user.get("email");
    let ticket = crate::fleet::sign(
        &crate::fleet::LaunchTicket {
            purpose: if member {
                "instance_login"
            } else if viewer {
                "instance_viewer"
            } else {
                "instance_support"
            }
            .into(),
            organisation_id: org,
            instance_id: instance,
            user_id: principal.user_id,
            email: email.clone(),
            display_name: user.get("display_name"),
            nonce: Uuid::new_v4(),
            expires_at: chrono::Utc::now().timestamp() + 60,
            management_session_id: principal.session_id,
        },
        &fleet_key,
    )?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "instance.open",
        instance,
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(json!({"url":url.as_str().trim_end_matches('/'),"ticket":ticket,"email":email})),
    ))
}

fn operation_available(instance: &Instance, action: InstanceAction) -> bool {
    if !PROVISIONING_ENABLED.get().copied().unwrap_or(true) {
        return false;
    }
    if action == InstanceAction::Delete {
        return deletion::enabled()
            && instance.backend == DeploymentBackend::Compose
            && instance.policy.minimum_isolation == Isolation::Container;
    }
    if action == InstanceAction::EnrollExecution {
        return execution_enrollment::enabled()
            && BACKUPS_ENABLED.get().copied().unwrap_or(false)
            && instance.backend == DeploymentBackend::Compose
            && instance.policy.minimum_isolation == Isolation::Container
            && instance.state == InstanceState::Stopped;
    }
    if matches!(action, InstanceAction::Upgrade | InstanceAction::Rollback) {
        return UPDATES_ENABLED.get().copied().unwrap_or(false)
            && BACKUPS_ENABLED.get().copied().unwrap_or(false)
            && instance.backend == DeploymentBackend::Compose
            && instance.policy.minimum_isolation == Isolation::Container
            && instance.state == InstanceState::Stopped
            && instance.current_release.is_some();
    }
    if matches!(action, InstanceAction::Backup | InstanceAction::Restore) {
        return BACKUPS_ENABLED.get().copied().unwrap_or(false)
            && instance.backend == DeploymentBackend::Compose
            && instance.policy.minimum_isolation == Isolation::Container
            && instance.state == InstanceState::Stopped;
    }
    // Backend contracts are broader than this first executable adapter. Do not
    // offer an unqualified action or a VM backend before its implementation is
    // ready, and never silently substitute containers for a VM requirement.
    instance.backend == DeploymentBackend::Compose
        && (instance.policy.minimum_isolation == Isolation::Container
            || matches!(action, InstanceAction::Stop | InstanceAction::Suspend))
        && matches!(
            action,
            InstanceAction::Provision
                | InstanceAction::Start
                | InstanceAction::Stop
                | InstanceAction::Suspend
                | InstanceAction::Resume
        )
}
async fn create_instance(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(request): Json<CreateInstance>,
) -> Result<Json<Value>, ApiError> {
    create_instance_with_host(state, principal, org, request, None).await
}
async fn create_instance_with_host(
    state: AppState,
    principal: Principal,
    org: Uuid,
    request: CreateInstance,
    host: Option<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let role = membership(&state, &principal, org, false).await?;
    if role == "auditor" {
        return Err(denied());
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    if host.is_some() {
        execution_admission::lock_global(&mut tx).await?;
    }
    if let Some(host) = host {
        let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_executor_hosts WHERE id=$1 AND trust_domain=$2 AND enabled)").bind(host).bind(org).fetch_one(&mut *tx).await?;
        if !allowed {
            return Err(denied());
        }
    }
    let parent: Value =
        sqlx::query_scalar("SELECT policy FROM management_organisations WHERE id=$1 AND archived_at IS NULL FOR UPDATE")
            .bind(org)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(denied)?;
    let parent: PolicyRevision =
        serde_json::from_value::<PolicyRevision>(parent)?.restricted_by(platform_ceiling());
    let policy = request.policy.unwrap_or_else(|| parent.clone());
    if host.is_none()
        && (policy.minimum_isolation == Isolation::VirtualMachine
            || request.backend == DeploymentBackend::Kubernetes)
    {
        return Err(invalid(
            "a VM or Kubernetes instance requires an original scoped executor host assignment",
        ));
    }
    provisioning_releases::validate_selection(
        &mut tx,
        request.release.as_deref(),
        request.backend,
        &policy,
        host,
    )
    .await?;
    policy.validate().map_err(invalid)?;
    if !policy.narrows(&parent) {
        return Err(invalid("instance policy exceeds organisation policy"));
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM management_instances WHERE organisation_id=$1 AND owner_user_id=$2 AND state<>'deleted'").bind(org).bind(principal.user_id).fetch_one(&mut *tx).await?;
    if count >= i64::from(parent.limits.instances) {
        return Err(ApiError::Quota("instance quota exhausted".into()));
    }
    // Platform administrators still explicitly join an organisation before owning an instance.
    let member: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM management_members WHERE organisation_id=$1 AND user_id=$2 AND role IN ('admin','member'))",
    )
    .bind(org)
    .bind(principal.user_id)
    .fetch_one(&mut *tx)
    .await?;
    if !member {
        return Err(invalid(
            "join the organisation before creating a personal instance",
        ));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO management_instances(id,organisation_id,owner_user_id,name,backend,policy,provisioning_release,executor_host_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(id).bind(org).bind(principal.user_id).bind(name(&request.name)?).bind(tag(request.backend)?).bind(serde_json::to_value(&policy)?).bind(&request.release).bind(host).execute(&mut *tx).await?;
    if let Some(host) = host {
        host_capacity::reserve_instance(&mut tx, org, host, id, &policy).await?;
    }
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "instance.create",
        id,
        json!({"backend":request.backend,"provisioning_release":request.release,"executor_host_id":host}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":"pending"})))
}
#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionRequest {
    action: InstanceAction,
    expected_revision: i64,
    idempotency_key: Uuid,
    backup_id: Option<Uuid>,
    release: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    qualification_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rollback_operation_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    confirmation_name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    purge_backups: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    reconcile_reservation_ids: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    acknowledge_execution_stop: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    acknowledge_execution_enrollment: bool,
}
async fn request_operation(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
    Json(request): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    instance_access(&state, &principal, org, id, true).await?;
    if !request.reconcile_reservation_ids.is_empty()
        || matches!(
            request.action,
            InstanceAction::Suspend | InstanceAction::Resume | InstanceAction::EnrollExecution
        )
    {
        membership(&state, &principal, org, true).await?;
    }
    if (request.action == InstanceAction::Rollback) != request.rollback_operation_id.is_some() {
        return Err(invalid("rollback requires its promotion operation ID"));
    }
    if request.action == InstanceAction::Rollback
        && (request.backup_id.is_some() || request.release.is_some())
    {
        return Err(invalid(
            "rollback backup and release are selected by the server",
        ));
    }
    if request.action == InstanceAction::Provision && request.release.is_some() {
        return Err(invalid(
            "initial release is selected when creating the instance",
        ));
    }
    if request.action == InstanceAction::Restore && request.backup_id.is_none() {
        return Err(invalid("restore requires a backup ID"));
    }
    if request.release.as_ref().is_some_and(|v| {
        v.is_empty()
            || v.len() > 80
            || !v
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    }) {
        return Err(invalid("invalid release identifier"));
    }
    if request.action == InstanceAction::Upgrade && request.release.is_none() {
        return Err(invalid("upgrade requires a release"));
    }
    if request.qualification_only && request.action != InstanceAction::Upgrade {
        return Err(invalid("qualification is only available for updates"));
    }
    let digest = hex::encode(Sha256::digest(serde_json::to_vec(
        &json!({"org":org,"instance":id,"request":request}),
    )?));
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    instance_members::authorize(&mut tx, &principal, org, id).await?;
    instance_imports::unlocked(&mut tx, id).await?;
    if !request.reconcile_reservation_ids.is_empty()
        || matches!(
            request.action,
            InstanceAction::Suspend | InstanceAction::Resume | InstanceAction::EnrollExecution
        )
    {
        sso::authorize(&mut tx, org, &principal).await?;
    }
    let value: Value = sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE organisation_id=$1 AND id=$2 FOR UPDATE").bind(org).bind(id).fetch_one(&mut *tx).await?;
    let current: Instance = serde_json::from_value(value.clone())?;
    let prior: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(o) FROM management_operations o WHERE actor_user_id=$1 AND idempotency_key=$2").bind(principal.user_id).bind(request.idempotency_key.to_string()).fetch_optional(&mut *tx).await?;
    if let Some(prior) = prior {
        if prior["request_digest"].as_str() != Some(&digest) {
            return Err(ApiError::Conflict(
                "idempotency key reused for a different request".into(),
            ));
        }
        return Ok(Json(json!({"id": prior["id"], "state": prior["state"]})));
    }
    if current.revision != request.expected_revision {
        return Err(ApiError::Conflict(
            "instance changed; reload before retrying".into(),
        ));
    }
    let profile = backend_profiles::current(&mut tx, org, id).await?;
    environments::instance_unlocked(&mut tx, id).await?;
    vm_snapshots::unlocked(&mut tx, id).await?;
    if !(operation_available(&current, request.action)
        || backend_profiles::available(&current, request.action, profile.as_ref()))
    {
        return Err(invalid(
            "this operation is not available for the selected backend and isolation profile",
        ));
    }
    execution_recovery::validate_request(&mut tx, &current, &request).await?;
    if profile.is_some() {
        execution_enrollment::validate_profiled(&value, &request, true)?;
    } else {
        execution_enrollment::validate(&value, &request)?;
    }
    if value["execution_required"] == true
        && matches!(
            request.action,
            InstanceAction::Start | InstanceAction::Resume
        )
        && !execution_admission::active()
    {
        return Err(ApiError::Unavailable(
            "activate central execution admission before starting an enrolled instance".into(),
        ));
    }
    if !(current.state.accepts(request.action)
        || request.action == InstanceAction::Stop
            && !request.reconcile_reservation_ids.is_empty()
            && current.state == InstanceState::Stopped)
    {
        return Err(ApiError::Conflict(
            "action is not allowed in the current instance state".into(),
        ));
    }
    if request.action == InstanceAction::Restore {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_backups WHERE id=$1 AND organisation_id=$2 AND instance_id=$3)")
            .bind(request.backup_id).bind(org).bind(id).fetch_one(&mut *tx).await?;
        if !exists {
            return Err(denied());
        }
    }
    let mut arguments = serde_json::to_value(&request)?;
    disk_quotas::bind_operation(&state, &mut tx, &principal, org, id, &mut arguments).await?;
    provisioning_releases::arguments(&mut tx, &current, &mut arguments).await?;
    deletion::arguments(&mut tx, &current, &request, &mut arguments).await?;
    if request.action == InstanceAction::Rollback {
        let point = rollbacks::point(&mut tx, org, id)
            .await?
            .filter(|p| Some(p.operation_id) == request.rollback_operation_id)
            .ok_or_else(|| invalid("rollback is unavailable for the installed generation"))?;
        arguments["source_release"] = json!(point.source_release);
        arguments["release"] = json!(point.release);
        arguments["backup_id"] = json!(point.backup_id);
    }
    if request.action == InstanceAction::Upgrade {
        let source = current
            .current_release
            .as_deref()
            .ok_or_else(|| invalid("installed release has not been reported"))?;
        let host = executor_hosts::instance_host(&mut tx, id).await?;
        releases::validate_selection_profiled(
            &mut tx,
            request.release.as_deref().unwrap_or(""),
            source,
            request.qualification_only,
            host,
            profile.as_ref(),
        )
        .await?;
        arguments["source_release"] = json!(source);
        arguments["qualification_only"] = json!(request.qualification_only);
    }
    if rollouts::reserved(&mut tx, id).await? {
        return Err(ApiError::Conflict(
            "instance is reserved by a rollout; cancel and settle its operations first".into(),
        ));
    }
    let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_operations WHERE instance_id=$1 AND state IN ('queued','running','reconciling'))").bind(id).fetch_one(&mut *tx).await?;
    if busy {
        return Err(ApiError::Conflict(
            "another instance operation is still active".into(),
        ));
    }
    let op = Uuid::new_v4();
    sqlx::query("INSERT INTO management_operations(id,organisation_id,instance_id,actor_user_id,action,arguments,idempotency_key,request_digest) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(op).bind(org).bind(id).bind(principal.user_id).bind(tag(request.action)?).bind(&arguments).bind(request.idempotency_key.to_string()).bind(digest).execute(&mut *tx).await?;
    if let Some(profile) = profile {
        sqlx::query("UPDATE management_operations SET backend_profile=$2 WHERE id=$1")
            .bind(op)
            .bind(profile)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE management_instances SET revision=revision+1 WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "instance.operation.request",
        id,
        json!({"operation_id":op,"action":request.action,"release":arguments["release"],"qualification_only":request.qualification_only,"rollback_operation_id":request.rollback_operation_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":op,"state":"queued"})))
}
async fn backups(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    instance_access(&state, &principal, org, id, false).await?;
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(b)-'purged_at' FROM management_backups b WHERE organisation_id=$1 AND instance_id=$2 AND purged_at IS NULL ORDER BY created_at DESC")
        .bind(org).bind(id).fetch_all(&state.pool).await?))
}

async fn operations(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    instance_access(&state, &principal, org, id, false).await?;
    sqlx::query("UPDATE management_operations SET state='reconciling',updated_at=now() WHERE organisation_id=$1 AND instance_id=$2 AND state='running' AND lease_expires_at<now()")
        .bind(org).bind(id).execute(&state.pool).await?;
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(o)-'lease_token'-'request_digest'-'execution_binding'-'execution_recovery_binding'-'executor_host_binding'-'backend_profile' FROM management_operations o WHERE organisation_id=$1 AND instance_id=$2 ORDER BY created_at DESC LIMIT 100").bind(org).bind(id).fetch_all(&state.pool).await?))
}

async fn request_recovery(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance, id)): Path<(Uuid, Uuid, Uuid)>,
    Json(request): Json<OperationRecoveryRequest>,
) -> Result<Json<Value>, ApiError> {
    let current = instance_access(&state, &principal, org, instance, true).await?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    let profile = backend_profiles::current(&mut tx, org, instance).await?;
    if !(current.backend == DeploymentBackend::Compose
        && current.policy.minimum_isolation == Isolation::Container
        || backend_profiles::available(&current, InstanceAction::Stop, profile.as_ref()))
    {
        return Err(invalid(
            "recovery requires the current original backend profile",
        ));
    }
    let row = sqlx::query("SELECT * FROM management_operations WHERE id=$1 AND organisation_id=$2 AND instance_id=$3 FOR UPDATE")
        .bind(id).bind(org).bind(instance).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    let action: String = row.try_get("action")?;
    if row.try_get::<Option<Value>, _>("backend_profile")? != profile {
        return Err(ApiError::Conflict(
            "original recovery backend profile changed".into(),
        ));
    }
    if instance_imports::requested(&json!(action)) {
        return Err(invalid("import reconciliation requires its original durable phase receipt; generic stopping recovery cannot replay import I/O"));
    }
    if row
        .try_get::<Option<Value>, _>("execution_recovery_binding")?
        .is_some()
    {
        sso::authorize(&mut tx, org, &principal).await?;
    }
    if action == "delete" {
        instance_members::authorize_deletion_recovery(&mut tx, &principal, org, instance).await?;
        if request.mode != OperationRecoveryMode::ContinueDeletion
            || request.confirmation_name.as_deref() != Some(current.name.as_str())
            || !request.purge_backups
            || !deletion::enabled()
        {
            return Err(invalid(
                "explicit permanent deletion acknowledgement is required to continue deletion",
            ));
        }
    } else if action == "enroll_execution" {
        sso::authorize(&mut tx, org, &principal).await?;
        if request.mode != OperationRecoveryMode::ContinueEnrollment
            || request.confirmation_name.is_some()
            || request.purge_backups
            || !execution_enrollment::enabled()
        {
            return Err(invalid("explicit enrollment reconciliation is required"));
        }
    } else {
        instance_members::authorize(&mut tx, &principal, org, instance).await?;
        if request.mode != OperationRecoveryMode::StopIfUnproven
            || request.confirmation_name.is_some()
            || request.purge_backups
        {
            return Err(invalid("this operation supports stopping recovery only"));
        }
    }
    if matches!(action.as_str(), "suspend" | "resume") {
        membership(&state, &principal, org, true).await?;
    }
    if row.try_get::<String, _>("state")? != "reconciling" {
        return Err(ApiError::Conflict(
            "operation does not require reconciliation".into(),
        ));
    }
    if matches!(action.as_str(), "delete" | "enroll_execution")
        || row.try_get::<Option<Uuid>, _>("recovery_requested_by")? != Some(principal.user_id)
    {
        sqlx::query("UPDATE management_operations SET recovery_requested_by=$2,recovery_requested_at=now(),updated_at=now() WHERE id=$1")
            .bind(id).bind(principal.user_id).execute(&mut *tx).await?;
        if matches!(action.as_str(), "delete" | "enroll_execution") {
            sqlx::query("UPDATE management_operations SET recovery_dispatched_at=NULL WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        append_audit(
            &mut tx,
            org,
            principal.user_id,
            "instance.operation.recovery_requested",
            instance,
            json!({"operation_id":id,"mode":request.mode}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":"reconciling"})))
}
#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateRequest {
    name: String,
    backend: EnvironmentBackend,
    isolation: Isolation,
    image_reference: String,
    #[serde(default)]
    request_id: Option<Uuid>,
}
async fn templates(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<Vec<Value>>, ApiError> {
    membership(&state, &principal, org, false).await?;
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(t) FROM management_templates t WHERE organisation_id=$1 ORDER BY name,revision DESC").bind(org).fetch_all(&state.pool).await?))
}
async fn create_template(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(request): Json<TemplateRequest>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    // Only immutable registry references. Provisioner qualification is separate from registration.
    let (repository, digest) = request
        .image_reference
        .split_once("@sha256:")
        .ok_or_else(|| invalid("templates require a digest-pinned image reference"))?;
    if repository.is_empty()
        || repository.starts_with(['-', '/'])
        || !repository
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/-:".contains(&b))
        || digest.len() != 64
        || !digest.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("invalid immutable image reference"));
    }
    let id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    sso::authorize(&mut tx, org, &principal).await?;
    let request_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(&request)?));
    if let Some(request_id) = request.request_id {
        if request_id.is_nil() {
            return Err(invalid("original template request identity required"));
        }
        if let Some(prior)=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(t) FROM management_templates t WHERE organisation_id=$1 AND registered_by=$2 AND request_id=$3").bind(org).bind(principal.user_id).bind(request_id).fetch_optional(&mut *tx).await? {
            if prior["request_sha256"]!=request_sha256{return Err(ApiError::Conflict("original template registration differs".into()));}
            return Ok(Json(json!({"id":prior["id"],"revision":prior["revision"],"enabled":prior["enabled"]})));
        }
    }
    sqlx::query("INSERT INTO management_templates(id,organisation_id,revision,name,backend,isolation,image_reference,registered_by,request_id,request_sha256) VALUES($1,$2,1,$3,$4,$5,$6,$7,$8,$9)")
        .bind(id).bind(org).bind(name(&request.name)?).bind(tag(request.backend)?).bind(tag(request.isolation)?).bind(&request.image_reference).bind(principal.user_id).bind(request.request_id).bind(request_sha256).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "template.register",
        id,
        json!({"backend":request.backend}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"revision":1,"enabled":false})))
}
async fn environments(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    instance_access(&state, &principal, org, id, false).await?;
    let administrative = platform(&state, &principal).await?
        || membership(&state, &principal, org, false).await? == "admin";
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(e) FROM management_environments e WHERE organisation_id=$1 AND instance_id=$2 AND ($4 OR management_environment_access_allowed(organisation_id,instance_id,id,$3,'viewer')) ORDER BY created_at").bind(org).bind(id).bind(principal.user_id).bind(administrative).fetch_all(&state.pool).await?))
}
async fn audit_log(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let role = membership(&state, &principal, org, false).await?;
    if role != "admin" && role != "auditor" {
        return Err(denied());
    }
    let mut tx = state.pool.begin().await?;
    let checkpoint = audit_lifecycle::lock(&mut tx, org).await?;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(a) FROM management_audit a WHERE organisation_id=$1 ORDER BY sequence DESC LIMIT 501").bind(org).fetch_all(&mut *tx).await?;
    let reached_base = rows.len() <= 500;
    let key = audit_key().map_err(ApiError::Internal)?;
    let mut verified = Vec::with_capacity(rows.len());
    let mut expected_older_digest: Option<String> = None;
    for row in rows {
        if expected_older_digest
            .as_deref()
            .is_some_and(|expected| row["digest"].as_str() != Some(expected))
        {
            return Err(ApiError::Conflict(
                "audit integrity verification failed".into(),
            ));
        }
        let record = verified_audit_record(row, org, &key)?;
        expected_older_digest = record["previous_digest"].as_str().map(str::to_owned);
        verified.push(record);
    }
    if reached_base {
        let digest = checkpoint["digest"]
            .as_str()
            .ok_or_else(|| invalid("invalid audit checkpoint"))?;
        if expected_older_digest.is_some_and(|older| older != digest) {
            return Err(ApiError::Conflict(
                "audit retained-prefix verification failed".into(),
            ));
        }
    }
    verified.truncate(500);
    tx.commit().await?;
    Ok(Json(verified))
}

fn verified_audit_record(mut row: Value, org: Uuid, key: &[u8]) -> Result<Value, ApiError> {
    let mismatch = || ApiError::Conflict("audit integrity verification failed".into());
    if row["payload"]["organisation_id"] != json!(org) {
        return Err(mismatch());
    }
    let previous = row["previous_digest"].as_str().ok_or_else(mismatch)?;
    let signature =
        hex::decode(row["digest"].as_str().ok_or_else(mismatch)?).map_err(|_| mismatch())?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| mismatch())?;
    mac.update(previous.as_bytes());
    mac.update(&serde_json::to_vec(&row["payload"])?);
    mac.verify_slice(&signature).map_err(|_| mismatch())?;
    // Render authenticated fields, not independently mutable index columns.
    for field in ["organisation_id", "actor_user_id", "action", "target_id"] {
        row[field] = row["payload"][field].clone();
    }
    row["created_at"] = row["payload"]["timestamp"].clone();
    Ok(row)
}
