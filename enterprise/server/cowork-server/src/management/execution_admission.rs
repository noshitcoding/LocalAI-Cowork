//! Global concurrency accounting across isolated databases. Capacity is released
//! only by an authenticated, matching completion, never by elapsed time.
use super::*;
use cowork_contracts::enterprise_execution::*;

#[derive(Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
struct Ceiling {
    revision: i64,
    platform_concurrent_runs: u32,
    organisation_concurrent_runs: u32,
    user_concurrent_runs: u32,
}
impl Default for Ceiling {
    fn default() -> Self {
        Self {
            revision: 1,
            platform_concurrent_runs: 40,
            organisation_concurrent_runs: 40,
            user_concurrent_runs: 2,
        }
    }
}
static CEILING: OnceLock<Ceiling> = OnceLock::new();
static ENABLED: OnceLock<bool> = OnceLock::new();

pub(super) fn initialize() -> anyhow::Result<()> {
    let ceiling: Ceiling = match std::env::var("COWORK_MANAGEMENT_EXECUTION_CEILING_FILE") {
        Ok(path) => serde_json::from_slice(&std::fs::read(path)?)?,
        Err(std::env::VarError::NotPresent) => Ceiling::default(),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        ceiling.revision > 0
            && ceiling.platform_concurrent_runs > 0
            && ceiling.platform_concurrent_runs <= 100_000
            && ceiling.organisation_concurrent_runs > 0
            && ceiling.organisation_concurrent_runs <= ceiling.platform_concurrent_runs
            && ceiling.user_concurrent_runs > 0
            && ceiling.user_concurrent_runs <= ceiling.organisation_concurrent_runs,
        "invalid execution capacity ceiling"
    );
    CEILING
        .set(ceiling)
        .map_err(|_| anyhow::anyhow!("execution admission already initialized"))?;
    ENABLED
        .set(
            std::env::var("COWORK_MANAGEMENT_EXECUTION_ADMISSION_ENABLED")
                .unwrap_or_else(|_| "false".into())
                .parse()?,
        )
        .map_err(|_| anyhow::anyhow!("execution admission already initialized"))
}
fn ceiling() -> &'static Ceiling {
    CEILING.get().expect("initialized before serving")
}

pub(super) async fn register(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('cowork:execution-admission:v1',0))",
    )
    .execute(&mut *tx)
    .await?;
    execution_coverage::register(&mut tx, active()).await?;
    if !active() {
        tx.commit().await?;
        return Ok(());
    }
    let last = sqlx::query("SELECT revision,settings FROM management_execution_ceiling_revisions ORDER BY revision DESC LIMIT 1").fetch_optional(&mut *tx).await?;
    let settings = serde_json::to_value(ceiling())?;
    if let Some(last) = &last {
        let previous: i64 = last.try_get("revision")?;
        anyhow::ensure!(
            previous <= ceiling().revision,
            "execution ceiling configuration is stale"
        );
        if previous == ceiling().revision {
            anyhow::ensure!(
                last.try_get::<Value, _>("settings")? == settings,
                "conflicting execution ceiling configuration at the same revision"
            );
            tx.commit().await?;
            return Ok(());
        }
    }
    let counts = sqlx::query("WITH held AS (SELECT organisation_id,instance_id,user_id FROM management_execution_reservations WHERE state='held' UNION ALL SELECT organisation_id,instance_id,user_id FROM management_environment_executions WHERE state IN ('queued','running','held')) SELECT (SELECT count(*) FROM held) AS platform,(SELECT COALESCE(max(n),0)::bigint FROM (SELECT count(*) n FROM held GROUP BY organisation_id) o) AS organisation,(SELECT COALESCE(max(n),0)::bigint FROM (SELECT count(*) n FROM held GROUP BY organisation_id,user_id) u) AS member")
        .fetch_one(&mut *tx).await?;
    anyhow::ensure!(
        counts.try_get::<i64, _>("platform")? <= i64::from(ceiling().platform_concurrent_runs)
            && counts.try_get::<i64, _>("organisation")?
                <= i64::from(ceiling().organisation_concurrent_runs)
            && counts.try_get::<i64, _>("member")? <= i64::from(ceiling().user_concurrent_runs),
        "outstanding reservations exceed the new execution ceiling"
    );
    sqlx::query(
        "INSERT INTO management_execution_ceiling_revisions(revision,settings) VALUES($1,$2)",
    )
    .bind(ceiling().revision)
    .bind(settings)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
async fn current_configuration(
    tx: &mut Transaction<'_, Postgres>,
    configured: &Ceiling,
) -> Result<(), ApiError> {
    let current: Option<Value> = sqlx::query_scalar("SELECT settings FROM management_execution_ceiling_revisions ORDER BY revision DESC LIMIT 1").fetch_optional(&mut **tx).await?;
    if current.as_ref() != Some(&serde_json::to_value(configured)?) {
        return Err(ApiError::Unavailable(
            "execution ceiling changed; this management process must be updated".into(),
        ));
    }
    Ok(())
}
fn enabled() -> Result<(), ApiError> {
    if !ENABLED.get().copied().unwrap_or(false) {
        return Err(ApiError::Unavailable(
            "central execution admission is not enabled".into(),
        ));
    }
    Ok(())
}

pub(super) fn active() -> bool {
    ENABLED.get().copied().unwrap_or(false)
}
pub(super) async fn ready(pool: &sqlx::PgPool) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    if active() {
        current_configuration(&mut tx, ceiling()).await?;
    } else {
        let configured:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_execution_activation) OR EXISTS(SELECT 1 FROM management_execution_ceiling_revisions)")
            .fetch_one(&mut *tx).await?;
        if configured {
            return Err(ApiError::Unavailable(
                "execution admission changed; update this management replica".into(),
            ));
        }
    }
    tx.commit().await?;
    Ok(())
}
async fn lock(tx: &mut Transaction<'_, Postgres>, org: Uuid) -> Result<(), ApiError> {
    // All releases, reservations and budget changes use the same order. The
    // global lock is required for a platform limit spanning organisations.
    lock_global(tx).await?;
    sqlx::query(
        "SELECT id FROM management_organisations WHERE id=$1 AND archived_at IS NULL FOR UPDATE",
    )
    .bind(org)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(denied)?;
    Ok(())
}
pub(super) async fn lock_global(tx: &mut Transaction<'_, Postgres>) -> Result<(), ApiError> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('cowork:execution-admission:v1',0))",
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}
async fn budget(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    configured: &Ceiling,
) -> Result<ExecutionBudget, ApiError> {
    current_configuration(tx, configured).await?;
    let value: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(l)-'organisation_id' FROM management_execution_limits l WHERE organisation_id=$1")
        .bind(org).fetch_optional(&mut **tx).await?;
    let mut value: ExecutionBudget = match value {
        Some(value) => serde_json::from_value(value)?,
        None => ExecutionBudget {
            revision: 0,
            organisation_concurrent_runs: configured.organisation_concurrent_runs,
            user_concurrent_runs: configured.user_concurrent_runs,
        },
    };
    value.organisation_concurrent_runs = value
        .organisation_concurrent_runs
        .min(configured.organisation_concurrent_runs);
    value.user_concurrent_runs = value
        .user_concurrent_runs
        .min(configured.user_concurrent_runs);
    Ok(value)
}
pub(super) async fn auxiliary_admission(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    instance: Uuid,
    user: Uuid,
    instance_limit: u32,
) -> Result<(), ApiError> {
    enabled()?;
    lock(tx, org).await?;
    let limits = budget(tx, org, ceiling()).await?;
    let counts=sqlx::query("SELECT count(*) AS platform,count(*) FILTER(WHERE organisation_id=$1) AS organisation,count(*) FILTER(WHERE organisation_id=$1 AND user_id=$2) AS member,count(*) FILTER(WHERE instance_id=$3) AS instance FROM (SELECT organisation_id,instance_id,user_id FROM management_execution_reservations WHERE state='held' UNION ALL SELECT organisation_id,instance_id,user_id FROM management_environment_executions WHERE state IN ('queued','running','held')) r").bind(org).bind(user).bind(instance).fetch_one(&mut **tx).await?;
    for (key, max) in [
        ("platform", ceiling().platform_concurrent_runs),
        ("organisation", limits.organisation_concurrent_runs),
        ("member", limits.user_concurrent_runs),
        ("instance", instance_limit),
    ] {
        if counts.try_get::<i64, _>(key)? >= i64::from(max) {
            return Err(ApiError::Unavailable(
                "environment_task_pool_exhausted".into(),
            ));
        }
    }
    Ok(())
}
pub(super) async fn inspect(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<ExecutionCapacity>, ApiError> {
    enabled()?;
    let role = membership(&state, &principal, org, false).await?;
    if role == "member" {
        return Err(denied());
    }
    let mut tx = state.pool.begin().await?;
    lock(&mut tx, org).await?;
    // Repeat the permission check after waiting for the organisation lock.
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u LEFT JOIN management_members m ON m.user_id=u.id AND m.organisation_id=$2 WHERE u.id=$1 AND u.deleted_at IS NULL AND (u.platform_admin OR m.role IN ('admin','auditor')))")
        .bind(principal.user_id).bind(org).fetch_one(&mut *tx).await?;
    if !allowed {
        return Err(denied());
    }
    let budget = budget(&mut tx, org, ceiling()).await?;
    let held: i64 = sqlx::query_scalar("SELECT count(*) FROM (SELECT organisation_id,user_id FROM management_execution_reservations WHERE state='held' UNION ALL SELECT organisation_id,user_id FROM management_environment_executions WHERE state IN ('queued','running','held')) r WHERE organisation_id=$1")
        .bind(org).fetch_one(&mut *tx).await?;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(r)-'state'-'completion'-'released_at'-'recovery_operation_id' FROM management_execution_reservations r WHERE organisation_id=$1 AND state='held' ORDER BY created_at,id LIMIT 100")
        .bind(org).fetch_all(&mut *tx).await?;
    let reservations = rows
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()?;
    tx.commit().await?;
    Ok(Json(ExecutionCapacity {
        organisation_id: org,
        budget,
        held_runs: held as u64,
        reservations,
    }))
}
pub(super) async fn update(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(request): Json<SetExecutionBudget>,
) -> Result<Json<ExecutionBudget>, ApiError> {
    enabled()?;
    if request.organisation_concurrent_runs == 0
        || request.organisation_concurrent_runs > ceiling().organisation_concurrent_runs
        || request.user_concurrent_runs == 0
        || request.user_concurrent_runs > ceiling().user_concurrent_runs
        || request.user_concurrent_runs > request.organisation_concurrent_runs
    {
        return Err(invalid("execution budget exceeds the operator ceiling"));
    }
    let mut tx = state.pool.begin().await?;
    lock(&mut tx, org).await?;
    sso::authorize(&mut tx, org, &principal).await?;
    let previous = budget(&mut tx, org, ceiling()).await?;
    if previous.revision != request.expected_revision {
        return Err(ApiError::Conflict(
            "execution budget changed; reload first".into(),
        ));
    }
    let held: i64 = sqlx::query_scalar("SELECT count(*) FROM (SELECT organisation_id,user_id FROM management_execution_reservations WHERE state='held' UNION ALL SELECT organisation_id,user_id FROM management_environment_executions WHERE state IN ('queued','running','held')) r WHERE organisation_id=$1")
        .bind(org).fetch_one(&mut *tx).await?;
    let largest: i64 = sqlx::query_scalar("SELECT COALESCE(max(n),0)::bigint FROM (SELECT count(*) n FROM (SELECT organisation_id,user_id FROM management_execution_reservations WHERE state='held' UNION ALL SELECT organisation_id,user_id FROM management_environment_executions WHERE state IN ('queued','running','held')) r WHERE organisation_id=$1 GROUP BY user_id) q")
        .bind(org).fetch_one(&mut *tx).await?;
    if held > i64::from(request.organisation_concurrent_runs)
        || largest > i64::from(request.user_concurrent_runs)
    {
        return Err(ApiError::Conflict(
            "outstanding reservations exceed the requested budget".into(),
        ));
    }
    sqlx::query("INSERT INTO management_execution_limits(organisation_id,organisation_concurrent_runs,user_concurrent_runs) VALUES($1,$2,$3) ON CONFLICT(organisation_id) DO UPDATE SET revision=management_execution_limits.revision+1,organisation_concurrent_runs=EXCLUDED.organisation_concurrent_runs,user_concurrent_runs=EXCLUDED.user_concurrent_runs")
        .bind(org).bind(i64::from(request.organisation_concurrent_runs)).bind(i64::from(request.user_concurrent_runs)).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "execution.budget.update",
        org,
        serde_json::to_value(&request)?,
    )
    .await?;
    let result = budget(&mut tx, org, ceiling()).await?;
    tx.commit().await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostCapacityAttestation {
    cpu_millis: u64,
    memory_mib: u64,
    reserved_cpu_millis: u64,
    reserved_memory_mib: u64,
}

fn qualified_host_capacity(value: Value) -> bool {
    let Ok(capacity) = serde_json::from_value::<HostCapacityAttestation>(value) else {
        return false;
    };
    capacity.cpu_millis > 0
        && capacity.cpu_millis <= 1_024_000
        && capacity.memory_mib > 0
        && capacity.memory_mib <= 4_194_304
        && capacity.reserved_cpu_millis < capacity.cpu_millis
        && capacity.reserved_memory_mib < capacity.memory_mib
}

async fn require_server_host(
    tx: &mut Transaction<'_, Postgres>,
    organisation: Uuid,
    host: Option<Uuid>,
    target: &cowork_contracts::ExecutorTarget,
) -> Result<(), ApiError> {
    if !matches!(target, cowork_contracts::ExecutorTarget::ServerLinux { .. }) {
        return Ok(());
    }
    let Some(host) = host else {
        return Ok(()); // Explicit legacy infrastructure remains unassigned.
    };
    let row = sqlx::query(
        "SELECT enabled, capacity FROM management_executor_hosts \
         WHERE id = $1 AND trust_domain = $2 FOR SHARE",
    )
    .bind(host)
    .bind(organisation)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::Unavailable("assigned execution host is unavailable".into()))?;
    if !row.try_get::<bool, _>("enabled")? {
        return Err(ApiError::Unavailable(
            "assigned execution host is unavailable".into(),
        ));
    }
    if !row
        .try_get::<Option<Value>, _>("capacity")?
        .is_some_and(qualified_host_capacity)
    {
        return Err(ApiError::Unavailable(
            "assigned execution host capacity is not qualified".into(),
        ));
    }
    // The production caller additionally requires the exact qualified static
    // allocation and scoped inventory through host_capacity::require_admission.
    Ok(())
}

pub(super) async fn reserve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ExecutionAdmissionRequest>,
) -> Result<Json<ExecutionAdmissionDecision>, ApiError> {
    enabled()?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    lock(&mut tx, request.organisation_id).await?;
    crate::fleet::verify_instance_key(
        &mut tx,
        &headers,
        request.organisation_id,
        request.instance_id,
    )
    .await?;
    let mfa = super::mfa::policy(&mut tx, request.organisation_id).await?;
    if mfa.required && request.mfa_contract != Some(1) {
        return Err(ApiError::MfaRequired(
            "executor instance does not enforce current organisation MFA".into(),
        ));
    }
    let (decision, created) = reserve_tx(&mut tx, &request, ceiling(), platform_ceiling()).await?;
    if created {
        append_audit(&mut tx, request.organisation_id, request.user_id, "execution.reserve", request.instance_id, json!({"reservation_id":decision.reservation_id,"run_id":request.run_id,"execution_epoch":request.execution_epoch,"ceiling_revision":ceiling().revision})).await?;
    }
    tx.commit().await?;
    Ok(Json(decision))
}

/// The same reservation transaction is used by the authenticated endpoint and
/// isolated concurrency qualification. Audit and commit remain in its caller.
async fn reserve_tx(
    tx: &mut Transaction<'_, Postgres>,
    request: &ExecutionAdmissionRequest,
    configured: &Ceiling,
    platform_policy: &PolicyRevision,
) -> Result<(ExecutionAdmissionDecision, bool), ApiError> {
    // Host control and instance operations take this gate before global/org
    // locks. Preserve that order before acquiring the shared host row lock.
    rollouts::lock(tx).await?;
    lock(tx, request.organisation_id).await?;
    let row = sqlx::query("SELECT i.policy,i.execution_epoch,i.execution_required,i.executor_host_id FROM management_instances i JOIN management_members m ON m.organisation_id=i.organisation_id AND m.user_id=$3 JOIN users u ON u.id=m.user_id WHERE i.organisation_id=$1 AND i.id=$2 AND i.state='ready' AND NOT EXISTS(SELECT 1 FROM management_instance_import_locks il WHERE il.instance_id=i.id) AND u.deleted_at IS NULL AND m.role IN ('admin','member') AND NOT EXISTS(SELECT 1 FROM management_operations o WHERE o.instance_id=i.id AND o.state IN ('queued','running','reconciling')) AND (i.owner_user_id=$3 OR EXISTS(SELECT 1 FROM management_instance_members im WHERE im.instance_id=i.id AND im.user_id=$3 AND im.role IN ('manager','member')))")
        .bind(request.organisation_id).bind(request.instance_id).bind(request.user_id).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    if row.try_get::<Uuid, _>("execution_epoch")? != request.execution_epoch {
        return Err(ApiError::Conflict(
            "execution generation is no longer current".into(),
        ));
    }
    // A held remote reservation may precede a failed local claim. Retrying it
    // after host deactivation must not grant authority for a new execution.
    // Leave its capacity held; exact confirmed completions still use release().
    require_server_host(
        tx,
        request.organisation_id,
        row.try_get("executor_host_id")?,
        &request.target,
    )
    .await?;
    let org_policy: Value =
        sqlx::query_scalar("SELECT policy FROM management_organisations WHERE id=$1")
            .bind(request.organisation_id)
            .fetch_one(&mut **tx)
            .await?;
    let effective_policy = serde_json::from_value::<PolicyRevision>(row.try_get("policy")?)?
        .restricted_by(&serde_json::from_value(org_policy)?)
        .restricted_by(platform_policy);
    if matches!(
        request.target,
        cowork_contracts::ExecutorTarget::ServerLinux { .. }
    ) {
        if let Some(host) = row.try_get::<Option<Uuid>, _>("executor_host_id")? {
            host_capacity::require_admission(
                tx,
                request.organisation_id,
                host,
                request.instance_id,
                request.execution_epoch,
                &effective_policy,
            )
            .await?;
        }
    }
    let target = serde_json::to_value(&request.target)?;
    let prior = sqlx::query("SELECT * FROM management_execution_reservations WHERE instance_id=$1 AND execution_epoch=$2 AND run_id=$3")
        .bind(request.instance_id).bind(request.execution_epoch).bind(request.run_id).fetch_optional(&mut **tx).await?;
    if let Some(prior) = prior {
        if prior.try_get::<Uuid, _>("user_id")? != request.user_id
            || prior.try_get::<Value, _>("target")? != target
        {
            return Err(ApiError::Conflict(
                "execution reservation binding changed".into(),
            ));
        }
        if prior.try_get::<String, _>("state")? != "held" {
            return Err(ApiError::Conflict(
                "completed execution cannot reserve again".into(),
            ));
        }
        return Ok((
            ExecutionAdmissionDecision {
                admitted: true,
                reservation_id: Some(prior.try_get("id")?),
                reason: None,
            },
            false,
        ));
    }
    let budget = budget(tx, request.organisation_id, configured).await?;
    environments::execution_unlocked(tx, request.instance_id).await?;
    let instance_limit = effective_policy.limits.concurrent_runs;
    let counts = sqlx::query("SELECT count(*) AS platform,count(*) FILTER(WHERE organisation_id=$1) AS organisation,count(*) FILTER(WHERE organisation_id=$1 AND user_id=$2) AS member,count(*) FILTER(WHERE instance_id=$3) AS instance FROM (SELECT organisation_id,instance_id,user_id FROM management_execution_reservations WHERE state='held' UNION ALL SELECT organisation_id,instance_id,user_id FROM management_environment_executions WHERE state IN ('queued','running','held')) r")
        .bind(request.organisation_id).bind(request.user_id).bind(request.instance_id).fetch_one(&mut **tx).await?;
    let constraints = [
        (
            "platform",
            configured.platform_concurrent_runs,
            ExecutionCapacityReason::Platform,
        ),
        (
            "organisation",
            budget.organisation_concurrent_runs,
            ExecutionCapacityReason::Organisation,
        ),
        (
            "member",
            budget.user_concurrent_runs,
            ExecutionCapacityReason::User,
        ),
        (
            "instance",
            instance_limit,
            ExecutionCapacityReason::Instance,
        ),
    ];
    for (name, limit, reason) in constraints {
        if counts.try_get::<i64, _>(name)? >= i64::from(limit) {
            return Ok((
                ExecutionAdmissionDecision {
                    admitted: false,
                    reservation_id: None,
                    reason: Some(reason),
                },
                false,
            ));
        }
    }
    let id = Uuid::new_v4();
    let fleet_key_version: i16 = if row.try_get::<bool, _>("execution_required")? {
        2
    } else {
        1
    };
    sqlx::query("INSERT INTO management_execution_reservations(id,organisation_id,instance_id,execution_epoch,run_id,user_id,target,fleet_key_version) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(id).bind(request.organisation_id).bind(request.instance_id).bind(request.execution_epoch).bind(request.run_id).bind(request.user_id).bind(target).bind(fleet_key_version).execute(&mut **tx).await?;
    Ok((
        ExecutionAdmissionDecision {
            admitted: true,
            reservation_id: Some(id),
            reason: None,
        },
        true,
    ))
}
pub(super) async fn release(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ExecutionReleaseRequest>,
) -> Result<Json<ExecutionReleaseAcknowledgement>, ApiError> {
    enabled()?;
    if !request.execution_stopped {
        return Err(invalid(
            "capacity remains reserved until execution is confirmed stopped",
        ));
    }
    let mut tx = state.pool.begin().await?;
    lock(&mut tx, request.organisation_id).await?;
    let row = sqlx::query("SELECT * FROM management_execution_reservations WHERE id=$1 AND organisation_id=$2 AND instance_id=$3 AND execution_epoch=$4 AND run_id=$5 FOR UPDATE")
        .bind(request.reservation_id).bind(request.organisation_id).bind(request.instance_id).bind(request.execution_epoch).bind(request.run_id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    crate::fleet::verify_execution_receipt_key(
        &mut tx,
        &headers,
        request.organisation_id,
        request.instance_id,
        request.execution_epoch,
        row.try_get("fleet_key_version")?,
    )
    .await?;
    let completion = serde_json::to_value(&request)?;
    if row.try_get::<String, _>("state")? == "released" {
        // A later genuine completion can acknowledge an already proven stop.
        // It never rewrites the operator evidence or frees capacity a second time.
        if row
            .try_get::<Option<Uuid>, _>("recovery_operation_id")?
            .is_none()
            && row.try_get::<Value, _>("completion")? != completion
        {
            return Err(ApiError::Conflict("completion receipt changed".into()));
        }
    } else {
        // Revocation, a new epoch, or deletion do not prevent an old executor
        // from settling its own exact reservation. They cannot acquire new ones.
        sqlx::query("UPDATE management_execution_reservations SET state='released',released_at=now(),completion=$2 WHERE id=$1")
            .bind(request.reservation_id).bind(completion).execute(&mut *tx).await?;
        append_audit(&mut tx, request.organisation_id, row.try_get("user_id")?, "execution.release", request.instance_id,
            json!({"reservation_id":request.reservation_id,"completion_id":request.completion_id,"outcome":request.outcome})).await?;
    }
    tx.commit().await?;
    Ok(Json(ExecutionReleaseAcknowledgement {
        reservation_id: request.reservation_id,
        released: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowork_contracts::ExecutorTarget;
    use std::sync::Arc;

    struct LoadCase {
        platform: u32,
        organisation: u32,
        shared_user: bool,
        instance: u32,
        expected_held: i64,
        reason: Option<ExecutionCapacityReason>,
    }

    struct LoadFixture {
        pool: sqlx::PgPool,
        admin: sqlx::PgPool,
        schema: String,
        configured: Arc<Ceiling>,
        policy: Arc<PolicyRevision>,
        requests: Vec<ExecutionAdmissionRequest>,
    }

    impl LoadFixture {
        async fn new(case: &LoadCase) -> Self {
            let url =
                std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated test database URL");
            let admin = sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await
                .unwrap();
            let schema = format!("cowork_admission_test_{}", Uuid::new_v4().simple());
            sqlx::query(&format!("CREATE SCHEMA {schema}"))
                .execute(&admin)
                .await
                .unwrap();
            let search_path = schema.clone();
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(12)
                .after_connect(move |connection, _| {
                    let search_path = search_path.clone();
                    Box::pin(async move {
                        // No public fallback: missing fixture tables must fail,
                        // never read or mutate any application schema.
                        sqlx::query("SELECT set_config('search_path',$1,false)")
                            .bind(search_path)
                            .execute(&mut *connection)
                            .await?;
                        sqlx::query("SET statement_timeout='20s'")
                            .execute(connection)
                            .await?;
                        Ok(())
                    })
                })
                .connect(&url)
                .await
                .unwrap();
            sqlx::raw_sql(r#"
                CREATE TABLE users(id uuid PRIMARY KEY,deleted_at timestamptz);
                CREATE TABLE management_organisations(id uuid PRIMARY KEY,policy jsonb NOT NULL,archived_at TIMESTAMPTZ);
                CREATE TABLE organisation_mfa_policies(organisation_id uuid PRIMARY KEY REFERENCES management_organisations(id),revision bigint NOT NULL DEFAULT 1,epoch uuid NOT NULL,required boolean NOT NULL DEFAULT false,max_age_seconds integer NOT NULL DEFAULT 43200,allow_validated_idp boolean NOT NULL DEFAULT true);
                CREATE TABLE management_members(organisation_id uuid,user_id uuid,role text,PRIMARY KEY(organisation_id,user_id));
                CREATE TABLE management_executor_hosts(id uuid PRIMARY KEY,trust_domain uuid,daemon_id text NOT NULL,operator_state_id uuid NOT NULL,revision bigint NOT NULL DEFAULT 1,enabled boolean NOT NULL DEFAULT true,capacity jsonb,UNIQUE(id,trust_domain));
                CREATE TABLE management_instances(id uuid PRIMARY KEY,organisation_id uuid,owner_user_id uuid,state text NOT NULL DEFAULT 'ready',policy jsonb,execution_epoch uuid,execution_required boolean NOT NULL DEFAULT false,executor_host_id uuid,execution_evidence jsonb,UNIQUE(organisation_id,id));
                CREATE TABLE management_instance_members(instance_id uuid,user_id uuid,role text,PRIMARY KEY(instance_id,user_id));
                CREATE TABLE management_instance_import_locks(instance_id uuid PRIMARY KEY,organisation_id uuid,import_id uuid);
                CREATE TABLE management_environments(instance_id uuid,state text);
                CREATE TABLE management_environment_jobs(instance_id uuid,state text);
                CREATE TABLE management_environment_executions(organisation_id uuid,instance_id uuid,user_id uuid,state text);
                CREATE TABLE management_operations(id uuid PRIMARY KEY DEFAULT gen_random_uuid(),instance_id uuid,state text,lease_expires_at timestamptz);
                CREATE TABLE management_execution_ceiling_revisions(revision bigint PRIMARY KEY,settings jsonb NOT NULL);
                CREATE TABLE management_execution_limits(organisation_id uuid PRIMARY KEY,revision bigint NOT NULL DEFAULT 1,organisation_concurrent_runs integer NOT NULL,user_concurrent_runs integer NOT NULL);
                CREATE TABLE management_execution_reservations(
                    id uuid PRIMARY KEY,organisation_id uuid NOT NULL,instance_id uuid NOT NULL,
                    execution_epoch uuid NOT NULL,run_id uuid NOT NULL,user_id uuid NOT NULL REFERENCES users(id),
                    target jsonb NOT NULL,state text NOT NULL DEFAULT 'held',completion jsonb,
                    created_at timestamptz NOT NULL DEFAULT now(),released_at timestamptz,recovery_operation_id uuid,
                    UNIQUE(instance_id,execution_epoch,run_id),
                    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id),
                    CHECK((state='held' AND completion IS NULL AND released_at IS NULL)
                       OR (state='released' AND completion IS NOT NULL AND released_at IS NOT NULL))
                );
            "#).execute(&pool).await.unwrap();
            sqlx::raw_sql(include_str!(
                "../../migrations/0055_host_capacity_allocations.sql"
            ))
            .execute(&pool)
            .await
            .unwrap();
            sqlx::raw_sql(include_str!(
                "../../migrations/0061_execution_reservation_fleet_version.sql"
            ))
            .execute(&pool)
            .await
            .unwrap();
            let configured = Arc::new(Ceiling {
                platform_concurrent_runs: case.platform,
                organisation_concurrent_runs: case.platform,
                ..Ceiling::default()
            });
            sqlx::query("INSERT INTO management_execution_ceiling_revisions VALUES($1,$2)")
                .bind(configured.revision)
                .bind(serde_json::to_value(&*configured).unwrap())
                .execute(&pool)
                .await
                .unwrap();
            let mut policy = PolicyRevision::default();
            policy.limits.concurrent_runs = 40;
            let organisations = [Uuid::new_v4(), Uuid::new_v4()];
            let hosts = [Uuid::new_v4(), Uuid::new_v4()];
            let operator_states = [Uuid::new_v4(), Uuid::new_v4()];
            let instances = [
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
            ];
            let epochs = [
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
            ];
            let common_user = Uuid::new_v4();
            sqlx::query("INSERT INTO users(id) VALUES($1)")
                .bind(common_user)
                .execute(&pool)
                .await
                .unwrap();
            for (index, organisation) in organisations.iter().enumerate() {
                sqlx::query("INSERT INTO management_organisations(id,policy) VALUES($1,$2)")
                    .bind(organisation)
                    .bind(serde_json::to_value(&policy).unwrap())
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query("INSERT INTO organisation_mfa_policies(organisation_id,epoch) VALUES($1,$2)")
                    .bind(organisation).bind(Uuid::new_v4()).execute(&pool).await.unwrap();
                sqlx::query("INSERT INTO management_executor_hosts(id,trust_domain,capacity,service_plan,daemon_id,operator_state_id) VALUES($1,$2,$3,$4,$5,$6)")
                    .bind(hosts[index]).bind(organisation)
                    .bind(json!({"cpu_millis":64000,"memory_mib":131072,"reserved_cpu_millis":1000,"reserved_memory_mib":1024}))
                    .bind(host_capacity::tests::service_plan()).bind(format!("daemon-fixture-{index}"))
                    .bind(operator_states[index]).execute(&pool).await.unwrap();
                host_capacity::tests::qualify_existing(
                    &pool,
                    &executor_hosts::HostBinding {
                        version: 1,
                        host_id: hosts[index],
                        trust_domain: *organisation,
                        daemon_id: format!("daemon-fixture-{index}"),
                        operator_state_id: operator_states[index],
                    },
                    &policy,
                )
                .await;
                sqlx::query("INSERT INTO management_execution_limits(organisation_id,organisation_concurrent_runs,user_concurrent_runs) VALUES($1,$2,2)")
                    .bind(organisation).bind(i64::from(case.organisation)).execute(&pool).await.unwrap();
            }
            let mut instance_policy = policy.clone();
            instance_policy.limits.concurrent_runs = case.instance;
            for index in 0..4 {
                sqlx::query("INSERT INTO management_instances(id,organisation_id,owner_user_id,policy,execution_epoch,executor_host_id,state) VALUES($1,$2,$3,$4,$5,$6,'pending')")
                    .bind(instances[index]).bind(organisations[index/2]).bind(common_user)
                    .bind(serde_json::to_value(&instance_policy).unwrap()).bind(epochs[index]).bind(hosts[index/2])
                    .execute(&pool).await.unwrap();
                let mut tx = pool.begin().await.unwrap();
                rollouts::lock(&mut tx).await.unwrap();
                lock(&mut tx, organisations[index / 2]).await.unwrap();
                host_capacity::reserve_instance(
                    &mut tx,
                    organisations[index / 2],
                    hosts[index / 2],
                    instances[index],
                    &instance_policy,
                )
                .await
                .unwrap();
                tx.commit().await.unwrap();
                sqlx::query("UPDATE management_instances SET state='ready',execution_evidence=jsonb_build_object('active_epoch',execution_epoch,'runtime',jsonb_build_object('version',1,'manifest_sha256',$2::text)) WHERE id=$1")
                    .bind(instances[index]).bind("a".repeat(64)).execute(&pool).await.unwrap();
            }
            for index in 0..2 {
                host_capacity::tests::qualify_existing(
                    &pool,
                    &executor_hosts::HostBinding {
                        version: 1,
                        host_id: hosts[index],
                        trust_domain: organisations[index],
                        daemon_id: format!("daemon-fixture-{index}"),
                        operator_state_id: operator_states[index],
                    },
                    &policy,
                )
                .await;
            }
            let mut requests = Vec::new();
            for index in 0..40 {
                let selected = index % 4;
                let organisation = organisations[selected / 2];
                let user = if case.shared_user {
                    common_user
                } else {
                    Uuid::new_v4()
                };
                sqlx::query("INSERT INTO users(id) VALUES($1) ON CONFLICT DO NOTHING")
                    .bind(user)
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query(
                    "INSERT INTO management_members VALUES($1,$2,'member') ON CONFLICT DO NOTHING",
                )
                .bind(organisation)
                .bind(user)
                .execute(&pool)
                .await
                .unwrap();
                sqlx::query("INSERT INTO management_instance_members VALUES($1,$2,'manager') ON CONFLICT DO NOTHING")
                    .bind(instances[selected]).bind(user).execute(&pool).await.unwrap();
                requests.push(ExecutionAdmissionRequest {
                    organisation_id: organisation,
                    instance_id: instances[selected],
                    execution_epoch: epochs[selected],
                    run_id: Uuid::new_v4(),
                    user_id: user,
                    target: ExecutorTarget::ServerLinux { pool_id: None },
                    mfa_contract: None,
                });
            }
            Self {
                pool,
                admin,
                schema,
                configured,
                policy: Arc::new(policy),
                requests,
            }
        }

        async fn close(self) {
            self.pool.close().await;
            // Only the generated UUID schema can be dropped, including when
            // a spawned assertion panics or a load scenario times out.
            Uuid::parse_str(self.schema.strip_prefix("cowork_admission_test_").unwrap()).unwrap();
            sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
                .execute(&self.admin)
                .await
                .unwrap();
            self.admin.close().await;
        }
    }

    async fn reserve_for_test(
        pool: &sqlx::PgPool,
        request: &ExecutionAdmissionRequest,
        configured: &Ceiling,
        policy: &PolicyRevision,
    ) -> Result<ExecutionAdmissionDecision, ApiError> {
        let mut tx = pool.begin().await?;
        let (decision, _) = reserve_tx(&mut tx, request, configured, policy).await?;
        tx.commit().await?;
        Ok(decision)
    }

    #[tokio::test]
    #[ignore = "requires an owned PostgreSQL fixture; actual Fleet HTTP, synthetic capacity inventory"]
    async fn postgres_fleet_http_reserve_release_preserve_original_key_version() {
        use std::{future::IntoFuture, io::Write, time::Duration};
        let fixture = LoadFixture::new(&LoadCase {
            platform: 40,
            organisation: 40,
            shared_user: false,
            instance: 40,
            expected_held: 40,
            reason: None,
        })
        .await;
        let pool = fixture.pool.clone();
        CEILING.set(Ceiling::default()).ok().unwrap();
        ENABLED.set(true).unwrap();
        PLATFORM_CEILING
            .set((*fixture.policy).clone())
            .ok()
            .unwrap();
        let keyfile =
            std::env::temp_dir().join(format!("cowork-fleet-http-key-{}", Uuid::new_v4()));
        struct Keys(
            std::path::PathBuf,
            Vec<(&'static str, Option<std::ffi::OsString>)>,
        );
        impl Drop for Keys {
            fn drop(&mut self) {
                for (name, value) in &self.1 {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _keys = Keys(
            keyfile.clone(),
            [
                "COWORK_PROVISIONER_TOKEN_FILE",
                "COWORK_MANAGEMENT_AUDIT_KEY_FILE",
            ]
            .into_iter()
            .map(|name| (name, std::env::var_os(name)))
            .collect(),
        );
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&keyfile)
            .unwrap();
        file.write_all(b"fixture-only-fleet-http-key-0000000000000000")
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&keyfile, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        for name in [
            "COWORK_PROVISIONER_TOKEN_FILE",
            "COWORK_MANAGEMENT_AUDIT_KEY_FILE",
        ] {
            std::env::set_var(name, &keyfile);
        }
        sqlx::raw_sql("CREATE TABLE management_audit(sequence bigserial PRIMARY KEY,organisation_id uuid,actor_user_id uuid,action text,target_id uuid,payload jsonb,previous_digest text,digest text)").execute(&pool).await.unwrap();
        let mut original = fixture.requests[0].clone();
        // The central endpoint also serves external executors. This fixture
        // focuses on its Fleet authority; no host/runtime stop is fabricated.
        original.target = ExecutorTarget::ManagedWindowsPool {
            pool_id: Uuid::new_v4(),
        };
        let result=tokio::spawn(async move {
            let state=AppState{pool:pool.clone(),management_mode:true,auth_secrets:None,bootstrap_token_digest:[0;32],bootstrap_user_id:Uuid::new_v4(),lease_seconds:15,
                server_capabilities:Arc::new(vec![]),object_store:None,runner:None,executor_hub:crate::executor_ws::ExecutorHub::new(None),api_replica:None,api_cluster:None,
                event_notifications:crate::event_notifications::EventNotifications::start(std::env::var("COWORK_TEST_DATABASE_URL").unwrap()),push:None,webauthn:None,oidc:None,
                organisation_oidc:Arc::new(sso::Registry::new()),directory:Arc::new(Default::default())};
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base=format!("http://{}",listener.local_addr().unwrap());
            let (stop,stopping)=tokio::sync::oneshot::channel();
            let mut api=tokio::spawn(axum::serve(listener,crate::management::provisioner_router().with_state(state)).with_graceful_shutdown(async move{let _=stopping.await;}).into_future());
            let client=reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(5)).build().unwrap();
            async fn call(client:&reqwest::Client,base:&str,path:&str,key:&str,body:Value,status:u16,label:&str)->Value {
                let response=client.post(format!("{base}/{path}")).header("x-cowork-instance-key",key).json(&body).send().await.unwrap();
                let actual=response.status().as_u16();let value:Value=response.json().await.unwrap();
                assert_eq!(actual,status,"{label}: {value}");println!("fleet-http-check: {label}");value
            }
            let legacy_key=crate::fleet::instance_key(original.organisation_id,original.instance_id).unwrap();
            let decision=call(&client,&base,"execution-reserve",&legacy_key,json!(original),200,"unenrolled legacy reserve").await;
            let legacy_id:Uuid=serde_json::from_value(decision["reservation_id"].clone()).unwrap();
            assert_eq!(sqlx::query_scalar::<_,i16>("SELECT fleet_key_version FROM management_execution_reservations WHERE id=$1").bind(legacy_id).fetch_one(&pool).await.unwrap(),1);
            let promoted=Uuid::new_v4();
            sqlx::query("UPDATE management_instances SET execution_required=true,execution_epoch=$2 WHERE id=$1").bind(original.instance_id).bind(promoted).execute(&pool).await.unwrap();
            let new_key=crate::fleet::epoch_key(original.organisation_id,original.instance_id,promoted).unwrap();
            let old_epoch_key=crate::fleet::epoch_key(original.organisation_id,original.instance_id,original.execution_epoch).unwrap();
            let release=ExecutionReleaseRequest{organisation_id:original.organisation_id,instance_id:original.instance_id,execution_epoch:original.execution_epoch,run_id:original.run_id,
                reservation_id:legacy_id,completion_id:Uuid::new_v4(),outcome:ExecutionCompletionOutcome::Completed,execution_stopped:true};
            call(&client,&base,"execution-release",&new_key,json!(release),401,"current v2 key cannot settle original v1 reservation").await;
            call(&client,&base,"execution-release",&old_epoch_key,json!(release),401,"v2 old epoch cannot replace original v1 authority").await;
            call(&client,&base,"execution-release",&legacy_key,json!(release),200,"original v1 completion remains valid after enrollment").await;
            call(&client,&base,"execution-release",&legacy_key,json!(release),200,"exact original completion replay").await;
            let mut changed=release.clone();changed.completion_id=Uuid::new_v4();
            call(&client,&base,"execution-release",&legacy_key,json!(changed),409,"changed completion receipt denied").await;
            let mut current=original.clone();current.run_id=Uuid::new_v4();current.execution_epoch=promoted;
            call(&client,&base,"execution-reserve",&legacy_key,json!(current),401,"legacy key cannot reserve enrolled work").await;
            call(&client,&base,"execution-reserve",&old_epoch_key,json!(current),401,"previous epoch cannot reserve new work").await;
            let wrong_scope=crate::fleet::epoch_key(original.organisation_id,Uuid::new_v4(),promoted).unwrap();
            call(&client,&base,"execution-reserve",&wrong_scope,json!(current),401,"source or foreign instance key cannot reserve target work").await;
            let decision=call(&client,&base,"execution-reserve",&new_key,json!(current),200,"current target epoch reserves").await;
            let current_id:Uuid=serde_json::from_value(decision["reservation_id"].clone()).unwrap();
            assert_eq!(sqlx::query_scalar::<_,i16>("SELECT fleet_key_version FROM management_execution_reservations WHERE id=$1").bind(current_id).fetch_one(&pool).await.unwrap(),2);
            assert!(sqlx::query("UPDATE management_execution_reservations SET fleet_key_version=1 WHERE id=$1").bind(current_id).execute(&pool).await.is_err());
            let next=Uuid::new_v4();
            sqlx::query("UPDATE management_instances SET execution_epoch=$2,state='deleted' WHERE id=$1").bind(original.instance_id).bind(next).execute(&pool).await.unwrap();
            let completion=ExecutionReleaseRequest{organisation_id:current.organisation_id,instance_id:current.instance_id,execution_epoch:current.execution_epoch,run_id:current.run_id,
                reservation_id:current_id,completion_id:Uuid::new_v4(),outcome:ExecutionCompletionOutcome::Canceled,execution_stopped:true};
            let next_key=crate::fleet::epoch_key(original.organisation_id,original.instance_id,next).unwrap();
            call(&client,&base,"execution-release",&next_key,json!(completion),401,"new epoch cannot acknowledge previous reservation").await;
            let mut unstopped=completion.clone();unstopped.execution_stopped=false;
            call(&client,&base,"execution-release",&new_key,json!(unstopped),422,"physical stop confirmation remains required").await;
            call(&client,&base,"execution-release",&new_key,json!(completion),200,"exact previous v2 reservation settles after promotion and deletion").await;
            call(&client,&base,"execution-reserve",&new_key,json!(current),401,"deleted old generation cannot reserve again").await;
            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM management_execution_reservations WHERE state='held'").fetch_one(&pool).await.unwrap(),0);
            stop.send(()).unwrap();tokio::time::timeout(Duration::from_secs(5),&mut api).await.unwrap().unwrap().unwrap();
            println!("fleet-http-owned-api: graceful shutdown joined");
        }).await;
        fixture.close().await;
        result.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL database with schema creation permission"]
    async fn postgres_forty_concurrent_requests_use_real_reservation_budget_and_lock_paths() {
        let cases = [
            LoadCase {
                platform: 40,
                organisation: 40,
                shared_user: false,
                instance: 40,
                expected_held: 40,
                reason: None,
            },
            LoadCase {
                platform: 10,
                organisation: 10,
                shared_user: false,
                instance: 40,
                expected_held: 10,
                reason: Some(ExecutionCapacityReason::Platform),
            },
            LoadCase {
                platform: 40,
                organisation: 6,
                shared_user: false,
                instance: 40,
                expected_held: 12,
                reason: Some(ExecutionCapacityReason::Organisation),
            },
            LoadCase {
                platform: 40,
                organisation: 40,
                shared_user: true,
                instance: 40,
                expected_held: 4,
                reason: Some(ExecutionCapacityReason::User),
            },
            LoadCase {
                platform: 40,
                organisation: 40,
                shared_user: false,
                instance: 2,
                expected_held: 8,
                reason: Some(ExecutionCapacityReason::Instance),
            },
        ];
        for case in cases {
            let fixture = LoadFixture::new(&case).await;
            let pool = fixture.pool.clone();
            let configured = fixture.configured.clone();
            let policy = fixture.policy.clone();
            let requests = fixture.requests.clone();
            let mut exercise = tokio::spawn(async move {
                let barrier = Arc::new(tokio::sync::Barrier::new(40));
                let mut tasks = tokio::task::JoinSet::new();
                for request in requests {
                    let (pool, configured, policy, barrier) = (
                        pool.clone(),
                        configured.clone(),
                        policy.clone(),
                        barrier.clone(),
                    );
                    tasks.spawn(async move {
                        // Wait before acquiring a pool connection so all forty
                        // callers contend without exhausting the pool at a barrier.
                        barrier.wait().await;
                        let decision = reserve_for_test(&pool, &request, &configured, &policy)
                            .await
                            .unwrap();
                        (request, decision)
                    });
                }
                let mut admitted = Vec::new();
                let mut refused = 0;
                while let Some(result) = tasks.join_next().await {
                    let (request, decision) = result.unwrap();
                    if decision.admitted {
                        assert!(decision.reservation_id.is_some());
                        assert!(decision.reason.is_none());
                        admitted.push((request, decision.reservation_id.unwrap()));
                    } else {
                        refused += 1;
                        assert!(decision.reservation_id.is_none());
                        assert_eq!(decision.reason, case.reason);
                        let serialized = serde_json::to_value(decision).unwrap();
                        assert_eq!(serialized["admitted"], false);
                        assert_eq!(serialized["reservation_id"], Value::Null);
                        assert_eq!(
                            serialized["reason"],
                            serde_json::to_value(case.reason).unwrap()
                        );
                        assert!(
                            serialized["reason"].is_string(),
                            "denial must retain its precise capacity code"
                        );
                    }
                }
                assert_eq!(admitted.len() as i64, case.expected_held);
                assert_eq!(refused, 40 - admitted.len());
                let counts = sqlx::query("SELECT (SELECT count(*) FROM management_execution_reservations WHERE state='held') AS platform,(SELECT max(n) FROM (SELECT count(*) n FROM management_execution_reservations WHERE state='held' GROUP BY organisation_id) q) AS organisation,(SELECT max(n) FROM (SELECT count(*) n FROM management_execution_reservations WHERE state='held' GROUP BY organisation_id,user_id) q) AS member,(SELECT max(n) FROM (SELECT count(*) n FROM management_execution_reservations WHERE state='held' GROUP BY instance_id) q) AS instance")
                    .fetch_one(&pool).await.unwrap();
                assert_eq!(counts.get::<i64, _>("platform"), case.expected_held);
                assert!(counts.get::<i64, _>("organisation") <= i64::from(case.organisation));
                assert!(counts.get::<i64, _>("member") <= 2);
                assert!(counts.get::<i64, _>("instance") <= i64::from(case.instance));
                let (request, reservation) = admitted[0].clone();
                // Concurrent retries keep one exact reservation even when all
                // applicable capacity limits are already exhausted.
                for _ in 0..16 {
                    let (pool, configured, policy, request) = (
                        pool.clone(),
                        configured.clone(),
                        policy.clone(),
                        request.clone(),
                    );
                    tasks.spawn(async move {
                        let decision = reserve_for_test(&pool, &request, &configured, &policy)
                            .await
                            .unwrap();
                        (request, decision)
                    });
                }
                while let Some(result) = tasks.join_next().await {
                    let (_, decision) = result.unwrap();
                    assert!(decision.admitted);
                    assert_eq!(decision.reservation_id, Some(reservation));
                    assert!(decision.reason.is_none());
                }
                let mut changed = request.clone();
                changed.target = ExecutorTarget::PersonalDevice {
                    device_id: Uuid::new_v4(),
                };
                assert!(matches!(
                    reserve_for_test(&pool, &changed, &configured, &policy).await,
                    Err(ApiError::Conflict(_))
                ));
                sqlx::query("UPDATE management_execution_reservations SET created_at=now()-interval '7 days'").execute(&pool).await.unwrap();
                if case.reason.is_none() {
                    let mut extra = request.clone();
                    extra.run_id = Uuid::new_v4();
                    let full = reserve_for_test(&pool, &extra, &configured, &policy)
                        .await
                        .unwrap();
                    assert!(!full.admitted);
                    assert_eq!(full.reason, Some(ExecutionCapacityReason::Platform));
                }
                sqlx::query("UPDATE management_executor_hosts SET enabled=false WHERE id=(SELECT executor_host_id FROM management_instances WHERE id=$1)")
                    .bind(request.instance_id).execute(&pool).await.unwrap();
                assert!(matches!(
                    reserve_for_test(&pool, &request, &configured, &policy).await,
                    Err(ApiError::Unavailable(_))
                ));
                sqlx::query("INSERT INTO management_operations(instance_id,state,lease_expires_at) VALUES($1,'running',now()-interval '1 hour')")
                    .bind(request.instance_id).execute(&pool).await.unwrap();
                assert!(matches!(
                    reserve_for_test(&pool, &request, &configured, &policy).await,
                    Err(ApiError::NotFound(_))
                ));
                let held:i64=sqlx::query_scalar("SELECT count(*) FROM management_execution_reservations WHERE state='held' AND completion IS NULL AND released_at IS NULL")
                    .fetch_one(&pool).await.unwrap();
                assert_eq!(
                    held, case.expected_held,
                    "age, expired operation lease, retries and host disable never release capacity"
                );
            });
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(45), &mut exercise).await;
            if result.is_err() {
                exercise.abort();
                let _ = exercise.await;
            }
            fixture.close().await;
            result
                .expect("isolated admission load scenario timed out")
                .unwrap();
        }
    }

    fn capacity() -> Value {
        json!({"cpu_millis":4000,"memory_mib":4096,
               "reserved_cpu_millis":500,"reserved_memory_mib":512})
    }

    #[test]
    fn host_attestation_requires_finite_capacity_and_usable_headroom() {
        assert!(qualified_host_capacity(capacity()));
        for invalid in [
            json!({}),
            json!({"cpu_millis":4000,"memory_mib":4096,"reserved_cpu_millis":4000,"reserved_memory_mib":512}),
            json!({"cpu_millis":4000,"memory_mib":4096,"reserved_cpu_millis":500,"reserved_memory_mib":4096}),
            json!({"cpu_millis":1_024_001,"memory_mib":4096,"reserved_cpu_millis":500,"reserved_memory_mib":512}),
            json!({"cpu_millis":4000,"memory_mib":4_194_305,"reserved_cpu_millis":500,"reserved_memory_mib":512}),
            json!({"cpu_millis":0,"memory_mib":4096,"reserved_cpu_millis":0,"reserved_memory_mib":512}),
            json!({"cpu_millis":4000,"memory_mib":0,"reserved_cpu_millis":500,"reserved_memory_mib":0}),
            json!({"cpu_millis":4000,"memory_mib":4096,"reserved_cpu_millis":-1,"reserved_memory_mib":512}),
            json!({"cpu_millis":4000,"memory_mib":4096,"reserved_cpu_millis":500,"reserved_memory_mib":512,"unqualified_extra":true}),
        ] {
            assert!(!qualified_host_capacity(invalid));
        }
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_server_host_admission_refuses_disabled_unqualified_or_foreign_hosts_without_releasing(
    ) {
        let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated test database URL");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut connection = pool.acquire().await.unwrap();
        // Only session-local records; no application data, schema or service
        // configuration is changed and no HTTP endpoint is contacted.
        sqlx::raw_sql(
            r#"
            CREATE TEMP TABLE management_executor_hosts (
                id uuid PRIMARY KEY, trust_domain uuid NOT NULL,
                enabled boolean NOT NULL DEFAULT true, capacity jsonb
            );
            CREATE TEMP TABLE management_execution_reservations (
                id uuid PRIMARY KEY, state text NOT NULL, completion jsonb
            );
        "#,
        )
        .execute(&mut *connection)
        .await
        .unwrap();
        drop(connection);
        let organisation = Uuid::new_v4();
        let host = Uuid::new_v4();
        let reservation = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO management_executor_hosts (id,trust_domain,capacity) VALUES ($1,$2,$3)",
        )
        .bind(host)
        .bind(organisation)
        .bind(capacity())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO management_execution_reservations (id,state) VALUES ($1,'held')")
            .bind(reservation)
            .execute(&pool)
            .await
            .unwrap();
        let target = ExecutorTarget::ServerLinux { pool_id: None };
        let mut tx = pool.begin().await.unwrap();
        require_server_host(&mut tx, organisation, Some(host), &target)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        sqlx::query("UPDATE management_executor_hosts SET enabled=false WHERE id=$1")
            .bind(host)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert!(matches!(
            require_server_host(&mut tx, organisation, Some(host), &target).await,
            Err(ApiError::Unavailable(_))
        ));
        // The production call runs before the idempotent held-reservation
        // branch. A refused retry cannot mint fresh execution authority, and
        // host deactivation is never accepted as proof of stopped execution.
        let held = sqlx::query(
            "SELECT state,completion FROM management_execution_reservations WHERE id=$1",
        )
        .bind(reservation)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(held.get::<String, _>("state"), "held");
        assert!(held.get::<Option<Value>, _>("completion").is_none());
        // Legacy unassigned infrastructure and external executors retain their
        // existing admission path rather than inheriting the scoped host gate.
        require_server_host(&mut tx, organisation, None, &target)
            .await
            .unwrap();
        require_server_host(
            &mut tx,
            organisation,
            Some(host),
            &ExecutorTarget::ManagedWindowsPool {
                pool_id: Uuid::new_v4(),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        sqlx::query("UPDATE management_executor_hosts SET enabled=true WHERE id=$1")
            .bind(host)
            .execute(&pool)
            .await
            .unwrap();
        for (org, selected) in [(Uuid::new_v4(), host), (organisation, Uuid::new_v4())] {
            let mut tx = pool.begin().await.unwrap();
            assert!(matches!(
                require_server_host(&mut tx, org, Some(selected), &target).await,
                Err(ApiError::Unavailable(_))
            ));
            tx.rollback().await.unwrap();
        }
        for attestation in [
            None,
            Some(json!({})),
            Some(json!({"cpu_millis":4000,"memory_mib":4096,
                        "reserved_cpu_millis":4000,"reserved_memory_mib":512})),
        ] {
            // Each attestation is a fresh immutable identity, matching the
            // registry contract; the test never changes an existing capacity.
            let unqualified = Uuid::new_v4();
            sqlx::query("INSERT INTO management_executor_hosts (id,trust_domain,capacity) VALUES ($1,$2,$3)")
                .bind(unqualified).bind(organisation).bind(attestation).execute(&pool).await.unwrap();
            let mut tx = pool.begin().await.unwrap();
            assert!(matches!(
                require_server_host(&mut tx, organisation, Some(unqualified), &target).await,
                Err(ApiError::Unavailable(_))
            ));
            tx.rollback().await.unwrap();
        }
        let held: i64 = sqlx::query_scalar("SELECT count(*) FROM management_execution_reservations WHERE id=$1 AND state='held' AND completion IS NULL")
            .bind(reservation).fetch_one(&pool).await.unwrap();
        assert_eq!(held, 1);
        pool.close().await;
    }
}
