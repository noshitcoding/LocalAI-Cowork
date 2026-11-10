//! Current member access to one original environment. Execution admission and
//! every later authority check use this policy; catalog grants cannot widen it.
use super::*;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mode {
    InheritInstance,
    Restricted,
}
impl Mode {
    fn text(self) -> &'static str {
        match self {
            Self::InheritInstance => "inherit_instance",
            Self::Restricted => "restricted",
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Role {
    Viewer,
    Runner,
}
impl Role {
    fn text(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Runner => "runner",
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Member {
    user_id: Uuid,
    role: Role,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Update {
    expected_revision: i64,
    mode: Mode,
    members: Vec<Member>,
}

pub(super) async fn require(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    instance: Uuid,
    environment: Uuid,
    user: Uuid,
    role: Role,
) -> Result<(), ApiError> {
    organisation_lifecycle::share_active(tx, org).await?;
    // Lock first, then evaluate in a fresh statement snapshot. A waiter cannot
    // authorise from the policy version preceding a concurrent replacement.
    sqlx::query("SELECT environment_id FROM management_environment_access_policies WHERE organisation_id=$1 AND instance_id=$2 AND environment_id=$3 FOR SHARE")
        .bind(org).bind(instance).bind(environment).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let allowed: bool =
        sqlx::query_scalar("SELECT management_environment_access_allowed($1,$2,$3,$4,$5)")
            .bind(org)
            .bind(instance)
            .bind(environment)
            .bind(user)
            .bind(role.text())
            .fetch_one(&mut **tx)
            .await?;
    if !allowed {
        return Err(denied());
    }
    Ok(())
}

async fn current(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    instance: Uuid,
    environment: Uuid,
) -> Result<Value, ApiError> {
    let policy = sqlx::query("SELECT revision,mode FROM management_environment_access_policies WHERE organisation_id=$1 AND instance_id=$2 AND environment_id=$3 FOR SHARE")
        .bind(org).bind(instance).bind(environment).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let members: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('user_id',user_id,'role',role) FROM management_environment_access_members WHERE organisation_id=$1 AND instance_id=$2 AND environment_id=$3 ORDER BY user_id")
        .bind(org).bind(instance).bind(environment).fetch_all(&mut **tx).await?;
    Ok(
        json!({"organisation_id":org,"instance_id":instance,"environment_id":environment,"revision":policy.get::<i64,_>("revision"),"mode":policy.get::<String,_>("mode"),"members":members}),
    )
}

async fn administrator(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    principal: &Principal,
    org: Uuid,
    instance: Uuid,
    environment: Uuid,
) -> Result<(), ApiError> {
    sso::authorize(tx, org, principal).await?;
    job_authority::capture(state, tx, principal, org).await?;
    sqlx::query("SELECT e.id FROM management_environments e JOIN management_instances i ON i.id=e.instance_id AND i.organisation_id=e.organisation_id WHERE e.organisation_id=$1 AND e.instance_id=$2 AND e.id=$3 AND i.state NOT IN ('deleted','deleting') AND e.state NOT IN ('deleted','deleting') FOR SHARE OF i,e")
        .bind(org).bind(instance).bind(environment).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    Ok(())
}

pub(super) async fn inspect(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance, environment)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    administrator(&state, &mut tx, &principal, org, instance, environment).await?;
    let value = current(&mut tx, org, instance, environment).await?;
    tx.commit().await?;
    Ok(Json(value))
}

pub(super) async fn update(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, instance, environment)): Path<(Uuid, Uuid, Uuid)>,
    Json(request): Json<Update>,
) -> Result<Json<Value>, ApiError> {
    let mut seen = HashSet::new();
    if request.expected_revision < 1
        || request.members.len() > 200
        || request
            .members
            .iter()
            .any(|m| m.user_id.is_nil() || !seen.insert(m.user_id))
        || matches!(request.mode, Mode::InheritInstance) && !request.members.is_empty()
    {
        return Err(invalid("use a current revision, distinct member IDs, and an empty member list when inheriting instance access"));
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    administrator(&state, &mut tx, &principal, org, instance, environment).await?;
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM management_environment_access_policies WHERE organisation_id=$1 AND instance_id=$2 AND environment_id=$3 FOR UPDATE")
        .bind(org).bind(instance).bind(environment).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    if revision != request.expected_revision {
        return Err(ApiError::Conflict(
            "reload environment access before saving".into(),
        ));
    }
    let unsettled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_environment_executions WHERE organisation_id=$1 AND instance_id=$2 AND environment_id=$3 AND state IN ('queued','running','held')) OR EXISTS(SELECT 1 FROM management_environment_jobs WHERE organisation_id=$1 AND instance_id=$2 AND (environment_id IS NULL OR environment_id=$3) AND state IN ('queued','running','held')) OR EXISTS(SELECT 1 FROM management_vm_jobs WHERE organisation_id=$1 AND instance_id=$2 AND state IN ('queued','running','held')) OR EXISTS(SELECT 1 FROM management_operations WHERE organisation_id=$1 AND instance_id=$2 AND state IN ('queued','running','reconciling')) OR EXISTS(SELECT 1 FROM management_rollout_targets WHERE organisation_id=$1 AND instance_id=$2 AND reserved)")
        .bind(org).bind(instance).bind(environment).fetch_one(&mut *tx).await?;
    if unsettled {
        return Err(ApiError::Conflict(
            "finish or reconcile outstanding runtime work before changing environment access"
                .into(),
        ));
    }
    for member in &request.members {
        // Assignments can narrow an instance grant, never create one. Runners
        // need an executing instance role; viewers may retain read-only access.
        let eligible: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_instances i JOIN management_members m ON m.organisation_id=i.organisation_id JOIN users u ON u.id=m.user_id WHERE i.organisation_id=$1 AND i.id=$2 AND m.user_id=$3 AND u.deleted_at IS NULL AND m.role IN ('admin','member','auditor') AND ($4='viewer' OR m.role IN ('admin','member')) AND (m.role='admin' OR i.owner_user_id=$3 OR EXISTS(SELECT 1 FROM management_instance_members im WHERE im.organisation_id=$1 AND im.instance_id=$2 AND im.user_id=$3 AND ($4='viewer' OR im.role IN ('manager','member')))))")
            .bind(org).bind(instance).bind(member.user_id).bind(member.role.text()).fetch_one(&mut *tx).await?;
        if !eligible {
            return Err(invalid("every selected member needs current access to this instance and an appropriate execution role"));
        }
    }
    sqlx::query("DELETE FROM management_environment_access_members WHERE organisation_id=$1 AND instance_id=$2 AND environment_id=$3")
        .bind(org).bind(instance).bind(environment).execute(&mut *tx).await?;
    for member in &request.members {
        sqlx::query("INSERT INTO management_environment_access_members(organisation_id,instance_id,environment_id,user_id,role) VALUES($1,$2,$3,$4,$5)")
            .bind(org).bind(instance).bind(environment).bind(member.user_id).bind(member.role.text()).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE management_environment_access_policies SET mode=$4,revision=revision+1,updated_by=$5,updated_at=clock_timestamp() WHERE organisation_id=$1 AND instance_id=$2 AND environment_id=$3")
        .bind(org).bind(instance).bind(environment).bind(request.mode.text()).bind(principal.user_id).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "environment.access.changed",
        environment,
        json!({"previous_revision":revision,"mode":request.mode,"members":request.members}),
    )
    .await?;
    let value = current(&mut tx, org, instance, environment).await?;
    tx.commit().await?;
    Ok(Json(value))
}

#[cfg(test)]
#[path = "environment_access_tests.rs"]
mod tests;
