//! Static instance maxima plus services, backed by a scoped operator report.
//! This is not an independent physical measurement by the manager. Charges never
//! expire and are reduced only by verified permanent deletion, never per run.
use super::*;
use executor_hosts::HostBinding;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ServiceCaps {
    cpu_millis: u64,
    memory_mib: u64,
    pids: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ServicePlan {
    api: ServiceCaps,
    worker: ServiceCaps,
    runner: ServiceCaps,
    #[serde(rename = "egress-proxy")]
    egress_proxy: ServiceCaps,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    postgres: Option<ServiceCaps>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    minio: Option<ServiceCaps>,
}
impl ServicePlan {
    pub fn validate(&self) -> Result<(), ApiError> {
        if self.postgres.is_some() != self.minio.is_some()
            || self.caps().iter().any(|v| {
                !(100..=32_000).contains(&v.cpu_millis)
                    || !(64..=32_768).contains(&v.memory_mib)
                    || !(16..=4096).contains(&v.pids)
            })
        {
            return Err(invalid("invalid executor service plan"));
        }
        Ok(())
    }
    fn caps(&self) -> Vec<&ServiceCaps> {
        let mut caps = vec![&self.api, &self.worker, &self.runner, &self.egress_proxy];
        caps.extend(self.postgres.iter());
        caps.extend(self.minio.iter());
        caps
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Pool {
    cpu_millis: u64,
    memory_mib: u64,
    concurrent_runs: u64,
}
impl Pool {
    fn policy(value: &PolicyRevision) -> Self {
        Self {
            cpu_millis: u64::from(value.limits.cpu_millis),
            memory_mib: u64::from(value.limits.memory_mib),
            concurrent_runs: u64::from(value.limits.concurrent_runs),
        }
    }
    fn within(&self, maximum: &Self) -> bool {
        self.cpu_millis > 0
            && self.memory_mib > 0
            && self.concurrent_runs > 0
            && self.cpu_millis <= maximum.cpu_millis
            && self.memory_mib <= maximum.memory_mib
            && self.concurrent_runs <= maximum.concurrent_runs
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Amount {
    cpu_millis: u64,
    memory_mib: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ResourcePlan {
    version: u32,
    instance_id: Uuid,
    services: ServicePlan,
    sandbox: Pool,
    total: Amount,
}
impl ResourcePlan {
    fn new(instance: Uuid, services: ServicePlan, sandbox: Pool) -> Self {
        let total = Amount {
            cpu_millis: sandbox.cpu_millis
                + services.caps().iter().map(|v| v.cpu_millis).sum::<u64>(),
            memory_mib: sandbox.memory_mib
                + services.caps().iter().map(|v| v.memory_mib).sum::<u64>(),
        };
        Self {
            version: 1,
            instance_id: instance,
            services,
            sandbox,
            total,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capacity {
    cpu_millis: u64,
    memory_mib: u64,
    reserved_cpu_millis: u64,
    reserved_memory_mib: u64,
}
#[derive(Deserialize)]
struct Host {
    id: Uuid,
    trust_domain: Uuid,
    daemon_id: String,
    operator_state_id: Uuid,
    enabled: bool,
    revision: i64,
    allocation_revision: i64,
    capacity: Option<Value>,
    service_plan: Option<ServicePlan>,
}
impl Host {
    fn binding(&self) -> HostBinding {
        HostBinding {
            version: 1,
            host_id: self.id,
            trust_domain: self.trust_domain,
            daemon_id: self.daemon_id.clone(),
            operator_state_id: self.operator_state_id,
        }
    }
    fn capacity(&self) -> Result<Capacity, ApiError> {
        let value: Capacity = serde_json::from_value(
            self.capacity
                .clone()
                .ok_or_else(|| unavailable("host_capacity_unqualified"))?,
        )
        .map_err(|_| unavailable("host_capacity_unqualified"))?;
        if value.cpu_millis == 0
            || value.cpu_millis > 1_024_000
            || value.memory_mib == 0
            || value.memory_mib > 4_194_304
            || value.reserved_cpu_millis >= value.cpu_millis
            || value.reserved_memory_mib >= value.memory_mib
        {
            return Err(unavailable("host_capacity_unqualified"));
        }
        Ok(value)
    }
    fn services(&self) -> Result<ServicePlan, ApiError> {
        let plan = self
            .service_plan
            .clone()
            .ok_or_else(|| unavailable("host_capacity_unqualified"))?;
        plan.validate()
            .map_err(|_| unavailable("host_capacity_unqualified"))?;
        Ok(plan)
    }
}
fn unavailable(reason: &str) -> ApiError {
    ApiError::Unavailable(reason.into())
}
fn digest(value: &impl serde::Serialize) -> Result<String, ApiError> {
    fn sorted(value: Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut entries: Vec<_> = map.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
            }
            Value::Array(values) => Value::Array(values.into_iter().map(sorted).collect()),
            other => other,
        }
    }
    Ok(
        Sha256::digest(serde_json::to_vec(&sorted(serde_json::to_value(value)?))?)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
async fn host(tx: &mut Transaction<'_, Postgres>, org: Uuid, id: Uuid) -> Result<Host, ApiError> {
    let value:Value=sqlx::query_scalar("SELECT to_jsonb(h) FROM management_executor_hosts h WHERE id=$1 AND trust_domain=$2 FOR UPDATE")
        .bind(id).bind(org).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    Ok(serde_json::from_value(value)?)
}
async fn missing_allocations(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    except: Option<Uuid>,
) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_instances i WHERE i.executor_host_id=$1 AND i.state<>'deleted' AND ($2::uuid IS NULL OR i.id<>$2) AND NOT EXISTS(SELECT 1 FROM management_host_allocations a WHERE a.instance_id=i.id AND a.host_id=$1 AND a.state<>'released'))")
        .bind(id).bind(except).fetch_one(&mut **tx).await?)
}
async fn fresh(
    tx: &mut Transaction<'_, Postgres>,
    host: &Host,
    inventory: bool,
) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_host_capacity_proofs p WHERE p.host_id=$1 AND p.host_revision=$2 AND p.qualified AND p.expires_at>clock_timestamp() AND (NOT $4 OR p.allocation_revision=$3))")
        .bind(host.id).bind(host.revision).bind(host.allocation_revision).bind(inventory).fetch_one(&mut **tx).await?)
}
async fn aggregate(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<Amount, ApiError> {
    let used=sqlx::query("SELECT COALESCE(sum(cpu_millis),0)::bigint cpu,COALESCE(sum(memory_mib),0)::bigint memory FROM management_host_allocations WHERE host_id=$1 AND state<>'released'")
        .bind(id).fetch_one(&mut **tx).await?;
    Ok(Amount {
        cpu_millis: u64::try_from(used.try_get::<i64, _>("cpu")?).unwrap_or(u64::MAX),
        memory_mib: u64::try_from(used.try_get::<i64, _>("memory")?).unwrap_or(u64::MAX),
    })
}
fn fits(capacity: &Capacity, used: &Amount, extra: &Amount) -> bool {
    used.cpu_millis
        .saturating_add(extra.cpu_millis)
        .saturating_add(capacity.reserved_cpu_millis)
        <= capacity.cpu_millis
        && used
            .memory_mib
            .saturating_add(extra.memory_mib)
            .saturating_add(capacity.reserved_memory_mib)
            <= capacity.memory_mib
}

/// Caller holds rollout -> global admission -> organisation locks. The instance
/// insert and this reservation commit together; every retained charge is counted.
pub(super) async fn reserve_instance(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    id: Uuid,
    instance: Uuid,
    policy: &PolicyRevision,
) -> Result<(), ApiError> {
    let host = host(tx, org, id).await?;
    if !host.enabled {
        return Err(unavailable("host_disabled"));
    }
    let capacity = host.capacity()?;
    let plan = ResourcePlan::new(instance, host.services()?, Pool::policy(policy));
    if !fresh(tx, &host, false).await? {
        return Err(unavailable("host_capacity_unqualified_or_stale"));
    }
    if missing_allocations(tx, id, Some(instance)).await? {
        return Err(unavailable("host_allocation_unknown"));
    }
    let unknown:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_host_allocations WHERE host_id=$1 AND state='unknown')")
        .bind(id).fetch_one(&mut **tx).await?;
    if unknown {
        return Err(unavailable("host_allocation_unknown"));
    }
    if !fits(&capacity, &aggregate(tx, id).await?, &plan.total) {
        return Err(ApiError::Quota("host_capacity_exhausted".into()));
    }
    sqlx::query("INSERT INTO management_host_allocations(instance_id,host_id,organisation_id,plan,plan_sha256,cpu_millis,memory_mib) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(instance).bind(id).bind(org).bind(serde_json::to_value(&plan)?).bind(digest(&plan)?)
        .bind(i64::try_from(plan.total.cpu_millis).map_err(|_|invalid("invalid allocation"))?)
        .bind(i64::try_from(plan.total.memory_mib).map_err(|_|invalid("invalid allocation"))?).execute(&mut **tx).await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChallengeRequest {
    executor_host_binding: HostBinding,
}
#[derive(Debug, Clone, Copy, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum InventoryState {
    Reserved,
    Applied,
    Unknown,
}
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct AllocationReport {
    instance_id: Uuid,
    allocation_revision: i64,
    state: InventoryState,
    execution_epoch: Uuid,
    manifest_sha256: Option<String>,
    resource_budget: Option<ResourcePlan>,
}
#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Attestation {
    version: u32,
    challenge_id: Uuid,
    executor_host_binding: HostBinding,
    observed_capacity: Amount,
    inventory_complete: bool,
    unknown_workloads: bool,
    allocations: Vec<AllocationReport>,
}
async fn inventory(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<Vec<Value>, ApiError> {
    Ok(sqlx::query_scalar("SELECT to_jsonb(a)||jsonb_build_object('instance_state',i.state,'execution_epoch',i.execution_epoch,'policy',i.policy,'org_policy',o.policy,'execution_evidence',i.execution_evidence) FROM management_host_allocations a JOIN management_instances i ON i.id=a.instance_id JOIN management_organisations o ON o.id=i.organisation_id WHERE a.host_id=$1 AND a.state<>'released' ORDER BY a.instance_id FOR UPDATE OF a")
        .bind(id).fetch_all(&mut **tx).await?)
}
fn expected_plan(row: &Value, platform: &PolicyRevision) -> Result<ResourcePlan, ApiError> {
    let initial: ResourcePlan = serde_json::from_value(row["plan"].clone())?;
    let policy: PolicyRevision = serde_json::from_value(row["policy"].clone())?;
    let org: PolicyRevision = serde_json::from_value(row["org_policy"].clone())?;
    let pool = Pool::policy(&policy.restricted_by(&org).restricted_by(platform));
    if !pool.within(&initial.sandbox) {
        return Err(unavailable("host_allocation_plan_mismatch"));
    }
    Ok(ResourcePlan::new(
        initial.instance_id,
        initial.services,
        pool,
    ))
}
pub(super) async fn challenge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChallengeRequest>,
) -> Result<Json<Value>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    execution_generation::lock_organisation(&mut tx, request.executor_host_binding.trust_domain)
        .await?;
    if !operator.current(&mut tx).await? {
        return Err(unavailable("host_disabled"));
    }
    let result = challenge_tx(&mut tx, &request.executor_host_binding, platform_ceiling()).await?;
    tx.commit().await?;
    Ok(Json(result))
}
async fn challenge_tx(
    tx: &mut Transaction<'_, Postgres>,
    binding: &HostBinding,
    platform: &PolicyRevision,
) -> Result<Value, ApiError> {
    let host = host(tx, binding.trust_domain, binding.host_id).await?;
    if host.binding() != *binding {
        return Err(denied());
    }
    if !host.enabled {
        return Err(unavailable("host_disabled"));
    }
    host.capacity()?;
    host.services()?;
    let rows = inventory(tx, host.id).await?;
    let mut allocations = Vec::new();
    for row in rows {
        allocations.push(json!({"instance_id":row["instance_id"],"allocation_revision":row["revision"],
            "execution_epoch":row["execution_epoch"],"instance_state":row["instance_state"],"plan":row["plan"],
            "plan_sha256":row["plan_sha256"],"expected_resource_budget":expected_plan(&row,platform)?,
            "expected_manifest_sha256":row["execution_evidence"]["runtime"]["manifest_sha256"]}));
    }
    sqlx::query("DELETE FROM management_host_capacity_challenges WHERE host_id=$1 OR expires_at<=clock_timestamp()")
        .bind(host.id).execute(&mut **tx).await?;
    let id = Uuid::new_v4();
    let deadline:chrono::DateTime<chrono::Utc>=sqlx::query_scalar("INSERT INTO management_host_capacity_challenges(id,host_id,host_revision,allocation_revision) VALUES($1,$2,$3,$4) RETURNING expires_at")
        .bind(id).bind(host.id).bind(host.revision).bind(host.allocation_revision).fetch_one(&mut **tx).await?;
    Ok(
        json!({"version":1,"challenge_id":id,"expires_at":deadline,"executor_host_binding":binding,
        "host_revision":host.revision,"allocation_revision":host.allocation_revision,"capacity":host.capacity,
        "service_plan":host.service_plan,"allocations":allocations}),
    )
}
pub(super) async fn attest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Attestation>,
) -> Result<Json<Value>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(Some(&request.executor_host_binding))?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    execution_generation::lock_organisation(&mut tx, request.executor_host_binding.trust_domain)
        .await?;
    operator.current(&mut tx).await?; // Revoked credentials may never report; disabled ones can report uncertainty.
    let result = attest_tx(&mut tx, &request, platform_ceiling()).await?;
    tx.commit().await?;
    Ok(Json(result))
}
async fn attest_tx(
    tx: &mut Transaction<'_, Postgres>,
    request: &Attestation,
    platform: &PolicyRevision,
) -> Result<Value, ApiError> {
    let host = host(
        tx,
        request.executor_host_binding.trust_domain,
        request.executor_host_binding.host_id,
    )
    .await?;
    if request.version != 1 || host.binding() != request.executor_host_binding {
        return Err(denied());
    }
    let challenge=sqlx::query("DELETE FROM management_host_capacity_challenges WHERE id=$1 AND host_id=$2 AND host_revision=$3 AND allocation_revision=$4 AND expires_at>clock_timestamp() RETURNING created_at")
        .bind(request.challenge_id).bind(host.id).bind(host.revision).bind(host.allocation_revision)
        .fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::Conflict("host capacity challenge expired or changed".into()))?;
    let started: chrono::DateTime<chrono::Utc> = challenge.try_get("created_at")?;
    let capacity = host.capacity()?;
    let rows = inventory(tx, host.id).await?;
    let ids: BTreeSet<_> = request.allocations.iter().map(|v| v.instance_id).collect();
    let mut reason = if !host.enabled {
        Some("host_disabled")
    } else if request.observed_capacity.cpu_millis < capacity.cpu_millis
        || request.observed_capacity.memory_mib < capacity.memory_mib
    {
        Some("host_capacity_unqualified")
    } else if !request.inventory_complete
        || request.unknown_workloads
        || request.allocations.len() > 10_000
        || ids.len() != request.allocations.len()
        || rows.len() != ids.len()
        || missing_allocations(tx, host.id, None).await?
    {
        Some("host_allocation_unknown")
    } else {
        None
    };
    if reason.is_none()
        && !fits(
            &capacity,
            &aggregate(tx, host.id).await?,
            &Amount {
                cpu_millis: 0,
                memory_mib: 0,
            },
        )
    {
        reason = Some("host_capacity_exhausted");
    }
    let services = host.services()?;
    if reason.is_none() {
        for row in &rows {
            let instance: Uuid = serde_json::from_value(row["instance_id"].clone())?;
            let Some(report) = request
                .allocations
                .iter()
                .find(|v| v.instance_id == instance)
            else {
                reason = Some("host_allocation_unknown");
                break;
            };
            let epoch: Uuid = serde_json::from_value(row["execution_epoch"].clone())?;
            let initial: ResourcePlan = serde_json::from_value(row["plan"].clone())?;
            if initial.services != services
                || initial.version != 1
                || initial.instance_id != instance
                || initial
                    != ResourcePlan::new(
                        instance,
                        initial.services.clone(),
                        initial.sandbox.clone(),
                    )
                || row["plan_sha256"] != json!(digest(&initial)?)
                || row["cpu_millis"] != json!(initial.total.cpu_millis)
                || row["memory_mib"] != json!(initial.total.memory_mib)
            {
                reason = Some("host_allocation_plan_mismatch");
                break;
            }
            if report.allocation_revision != row["revision"].as_i64().unwrap_or(0)
                || report.execution_epoch != epoch
            {
                reason = Some("allocation_generation_changed");
                break;
            }
            match report.state {
                InventoryState::Unknown => {
                    reason = Some("host_allocation_unknown");
                    break;
                }
                InventoryState::Reserved => {
                    if row["instance_state"] != "pending"
                        || !row["execution_evidence"].is_null()
                        || report.manifest_sha256.is_some()
                        || report.resource_budget.is_some()
                    {
                        reason = Some("host_allocation_unknown");
                        break;
                    }
                }
                InventoryState::Applied => {
                    let expected = match expected_plan(row, platform) {
                        Ok(plan) => plan,
                        Err(_) => {
                            reason = Some("host_allocation_plan_mismatch");
                            break;
                        }
                    };
                    if expected.services != services
                        || report.resource_budget.as_ref() != Some(&expected)
                    {
                        reason = Some("host_allocation_plan_mismatch");
                        break;
                    }
                    if !report.manifest_sha256.as_deref().is_some_and(valid_digest)
                        || row["execution_evidence"]["active_epoch"] != json!(epoch)
                        || row["execution_evidence"]["runtime"]["version"] != 1
                        || row["execution_evidence"]["runtime"]["manifest_sha256"]
                            != json!(report.manifest_sha256)
                    {
                        reason = Some("allocation_generation_changed");
                        break;
                    }
                }
            }
        }
    }
    if reason.is_none() {
        for report in &request.allocations {
            let applied = matches!(report.state, InventoryState::Applied);
            sqlx::query("UPDATE management_host_allocations SET state=$2,verified_epoch=$3,verified_manifest_sha256=$4,verified_revision=$5,verified_budget=$6 WHERE instance_id=$1 AND host_id=$7 AND state<>'released'")
                .bind(report.instance_id).bind(if applied {"verified"}else{"reserved"}).bind(applied.then_some(report.execution_epoch))
                .bind(&report.manifest_sha256).bind(applied.then_some(report.allocation_revision)).bind(report.resource_budget.as_ref().map(serde_json::to_value).transpose()?)
                .bind(host.id).execute(&mut **tx).await?;
        }
    }
    // Invalid complete observations are durable fail-closed results. A replayed
    // challenge cannot refresh the TTL; the bound is the DB challenge start.
    sqlx::query("INSERT INTO management_host_capacity_proofs(host_id,host_revision,allocation_revision,qualified,reason,observed_capacity,inventory_sha256,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8+interval '120 seconds') ON CONFLICT(host_id) DO UPDATE SET host_revision=EXCLUDED.host_revision,allocation_revision=EXCLUDED.allocation_revision,qualified=EXCLUDED.qualified,reason=EXCLUDED.reason,observed_capacity=EXCLUDED.observed_capacity,inventory_sha256=EXCLUDED.inventory_sha256,checked_at=clock_timestamp(),expires_at=EXCLUDED.expires_at")
        .bind(host.id).bind(host.revision).bind(host.allocation_revision).bind(reason.is_none()).bind(reason)
        .bind(serde_json::to_value(&request.observed_capacity)?).bind(digest(&request.allocations)?).bind(started).execute(&mut **tx).await?;
    Ok(json!({"qualified":reason.is_none(),"reason":reason}))
}

pub(super) async fn require_admission(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    id: Uuid,
    instance: Uuid,
    epoch: Uuid,
    policy: &PolicyRevision,
) -> Result<(), ApiError> {
    let host = host(tx, org, id).await?;
    if !host.enabled {
        return Err(unavailable("host_disabled"));
    }
    host.capacity()?;
    host.services()?;
    if !fits(
        &host.capacity()?,
        &aggregate(tx, id).await?,
        &Amount {
            cpu_millis: 0,
            memory_mib: 0,
        },
    ) {
        return Err(unavailable("host_capacity_exhausted"));
    }
    if !fresh(tx, &host, true).await? {
        return Err(unavailable("host_allocation_unqualified_or_stale"));
    }
    if missing_allocations(tx, id, None).await? {
        return Err(unavailable("host_allocation_unknown"));
    }
    let row=sqlx::query("SELECT a.*,i.execution_evidence FROM management_host_allocations a JOIN management_instances i ON i.id=a.instance_id WHERE a.instance_id=$1 AND a.host_id=$2 AND a.state='verified' AND a.verified_revision=a.revision AND a.verified_epoch=$3 FOR SHARE OF a")
        .bind(instance).bind(id).bind(epoch).fetch_optional(&mut **tx).await?.ok_or_else(||unavailable("host_allocation_unqualified"))?;
    let plan: ResourcePlan = serde_json::from_value(row.try_get("plan")?)?;
    let pool = Pool::policy(policy);
    if !pool.within(&plan.sandbox)
        || row.try_get::<Option<Value>, _>("verified_budget")?
            != Some(serde_json::to_value(ResourcePlan::new(
                instance,
                plan.services,
                pool,
            ))?)
    {
        return Err(unavailable("host_allocation_plan_mismatch"));
    }
    let evidence: Option<Value> = row.try_get("execution_evidence")?;
    let evidence = evidence.ok_or_else(|| unavailable("allocation_generation_changed"))?;
    if evidence["active_epoch"] != json!(epoch)
        || evidence["runtime"]["version"] != 1
        || evidence["runtime"]["manifest_sha256"]
            != json!(row.try_get::<Option<String>, _>("verified_manifest_sha256")?)
    {
        return Err(unavailable("allocation_generation_changed"));
    }
    Ok(())
}

/// Policy changes retain the frozen maximum. They invalidate applied coverage.
pub(super) async fn invalidate_policy(
    tx: &mut Transaction<'_, Postgres>,
    instance: Uuid,
) -> Result<(), ApiError> {
    let assignment: Option<(Option<Uuid>, Uuid)> = sqlx::query_as(
        "SELECT executor_host_id,organisation_id FROM management_instances WHERE id=$1",
    )
    .bind(instance)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((Some(id), org)) = assignment else {
        return Ok(());
    };
    host(tx, org, id).await?; // Keep host before allocation row locks.
    sqlx::query("UPDATE management_host_allocations SET revision=revision+1,state='unknown',verified_epoch=NULL,verified_revision=NULL,verified_budget=NULL,verified_manifest_sha256=NULL WHERE instance_id=$1 AND state<>'released'")
        .bind(instance).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn prepare_operation(
    tx: &mut Transaction<'_, Postgres>,
    operation: &mut Value,
    instance: &Value,
) -> Result<(), ApiError> {
    if instance["executor_host_id"].is_null() {
        return Ok(());
    }
    if matches!(
        operation["action"].as_str(),
        Some(
            "provision"
                | "start"
                | "resume"
                | "import_quiesce"
                | "import_export"
                | "import_restore"
                | "import_qualify"
        )
    ) {
        let host = host(
            tx,
            serde_json::from_value(instance["organisation_id"].clone())?,
            serde_json::from_value(instance["executor_host_id"].clone())?,
        )
        .await?;
        if !host.enabled || !fresh(tx, &host, false).await? {
            return Err(unavailable("host_capacity_unqualified_or_stale"));
        }
    }
    let id: Uuid = serde_json::from_value(instance["id"].clone())?;
    let row:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(a) FROM management_host_allocations a WHERE instance_id=$1 AND state<>'released'")
        .bind(id).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        if matches!(
            operation["action"].as_str(),
            Some("stop" | "suspend" | "delete")
        ) {
            return Ok(());
        }
        return Err(unavailable("host_allocation_unknown"));
    };
    let binding = json!({"version":1,"host_id":row["host_id"],"organisation_id":row["organisation_id"],"instance_id":id,
        "allocation_revision":row["revision"],"plan_sha256":row["plan_sha256"],"execution_epoch":instance["execution_epoch"]});
    let op: Uuid = serde_json::from_value(operation["id"].clone())?;
    let changed=sqlx::query("UPDATE management_operations SET host_allocation_binding=$2 WHERE id=$1 AND host_allocation_binding IS NULL")
        .bind(op).bind(&binding).execute(&mut **tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError::Conflict(
            "host allocation operation binding is already frozen".into(),
        ));
    }
    operation["host_allocation_binding"] = binding;
    attach_frozen_operation(tx, operation).await
}

/// Reconstruct the original dispatch envelope for recovery without minting a
/// new revision/epoch binding. Released allocations retain their immutable plan.
pub(super) async fn attach_frozen_operation(
    tx: &mut Transaction<'_, Postgres>,
    operation: &mut Value,
) -> Result<(), ApiError> {
    let binding = operation["host_allocation_binding"].clone();
    if binding.is_null() {
        return Ok(());
    }
    if binding["version"] != 1 || binding["instance_id"] != operation["instance_id"] {
        return Err(invalid("invalid frozen host allocation operation binding"));
    }
    let instance: Uuid = serde_json::from_value(operation["instance_id"].clone())?;
    let plan: Option<Value> = sqlx::query_scalar("SELECT plan FROM management_host_allocations WHERE instance_id=$1 AND host_id::text=$2 AND organisation_id::text=$3 AND plan_sha256=$4")
        .bind(instance).bind(binding["host_id"].as_str()).bind(binding["organisation_id"].as_str())
        .bind(binding["plan_sha256"].as_str()).fetch_optional(&mut **tx).await?;
    let plan = plan.ok_or_else(|| unavailable("host_allocation_unknown"))?;
    if json!(digest(&plan)?) != binding["plan_sha256"] {
        return Err(unavailable("host_allocation_plan_mismatch"));
    }
    operation["host_allocation"] = json!({"binding":binding,"plan":plan});
    Ok(())
}

pub(super) fn validate_operation_receipt(
    row: &sqlx::postgres::PgRow,
    result: &Value,
) -> Result<(), ApiError> {
    if let Some(binding) = row.try_get::<Option<Value>, _>("host_allocation_binding")? {
        if result["host_allocation"] != binding {
            return Err(invalid("host allocation receipt binding required"));
        }
    }
    Ok(())
}

pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    row: &sqlx::postgres::PgRow,
    succeeded: bool,
    result: &Value,
) -> Result<(), ApiError> {
    validate_operation_receipt(row, result)?;
    let binding: Option<Value> = row.try_get("host_allocation_binding")?;
    let Some(binding) = binding else {
        return Ok(());
    };
    let instance: Uuid = row.try_get("instance_id")?;
    let action: String = row.try_get("action")?;
    if action != "delete" {
        // Import stop/export leave an already stopped generation unchanged;
        // qualification only observes the restored candidate. Restore alone
        // invalidates its old epoch coverage and advances the exact revision.
        if !matches!(
            action.as_str(),
            "backup" | "import_quiesce" | "import_export" | "import_qualify"
        ) {
            invalidate_policy(tx, instance).await?;
        }
        return Ok(());
    }
    if !succeeded
        || result["deletion_verified"] != true
        || result["receipt_id"] != json!(row.try_get::<Uuid, _>("id")?)
        || result["instance_id"] != json!(instance)
        || !result["deletion_inventory_sha256"]
            .as_str()
            .is_some_and(valid_digest)
    {
        return Err(invalid("host allocation deletion proof required"));
    }
    let held:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_execution_reservations WHERE instance_id=$1 AND state='held')")
        .bind(instance).fetch_one(&mut **tx).await?;
    if held {
        return Err(ApiError::Conflict(
            "settle outstanding executions before releasing host allocation".into(),
        ));
    }
    let changed=sqlx::query("UPDATE management_host_allocations a SET state='released',release_proof=$2,released_at=clock_timestamp() FROM management_instances i WHERE a.instance_id=$1 AND i.id=a.instance_id AND a.state<>'released' AND a.host_id::text=$3 AND a.organisation_id::text=$4 AND a.revision=$5 AND a.plan_sha256=$6 AND i.execution_epoch::text=$7")
        .bind(instance).bind(json!({"operation_id":row.try_get::<Uuid,_>("id")?,"binding":binding,"inventory_sha256":result["deletion_inventory_sha256"]}))
        .bind(binding["host_id"].as_str()).bind(binding["organisation_id"].as_str()).bind(binding["allocation_revision"].as_i64())
        .bind(binding["plan_sha256"].as_str()).bind(binding["execution_epoch"].as_str()).execute(&mut **tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError::Conflict(
            "host allocation changed during deletion".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::{sync::Barrier, task::JoinSet};

    pub(in crate::management) fn service_plan() -> Value {
        json!({"api":{"cpu_millis":1000,"memory_mib":512,"pids":256},
            "worker":{"cpu_millis":500,"memory_mib":512,"pids":256},
            "runner":{"cpu_millis":500,"memory_mib":256,"pids":128},
            "egress-proxy":{"cpu_millis":250,"memory_mib":128,"pids":64}})
    }
    fn policy() -> PolicyRevision {
        let mut value = PolicyRevision::default();
        value.limits.cpu_millis = 500;
        value.limits.memory_mib = 512;
        value.limits.concurrent_runs = 40;
        value.limits.instances = 1000;
        value
    }
    async fn context(pool: &sqlx::PgPool, org: Uuid) -> Transaction<'_, Postgres> {
        let mut tx = pool.begin().await.unwrap();
        rollouts::lock(&mut tx).await.unwrap();
        execution_admission::lock_global(&mut tx).await.unwrap();
        execution_generation::lock_organisation(&mut tx, org)
            .await
            .unwrap();
        tx
    }
    /// Fixture observations are fed through the production challenge/attestation
    /// core. They are test data, not a claim to have measured a physical daemon.
    pub(in crate::management) async fn qualify_existing(
        pool: &sqlx::PgPool,
        binding: &HostBinding,
        platform: &PolicyRevision,
    ) -> Value {
        let request = report(pool, binding, platform).await;
        let mut tx = context(pool, binding.trust_domain).await;
        let result = attest_tx(&mut tx, &request, platform).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(result["qualified"], true, "{result}");
        result
    }
    async fn report(
        pool: &sqlx::PgPool,
        binding: &HostBinding,
        platform: &PolicyRevision,
    ) -> Attestation {
        let mut tx = context(pool, binding.trust_domain).await;
        let challenge = challenge_tx(&mut tx, binding, platform).await.unwrap();
        let rows = inventory(&mut tx, binding.host_id).await.unwrap();
        let allocations = rows
            .iter()
            .map(|row| {
                let pending = row["instance_state"] == "pending";
                AllocationReport {
                    instance_id: serde_json::from_value(row["instance_id"].clone()).unwrap(),
                    allocation_revision: row["revision"].as_i64().unwrap(),
                    state: if pending {
                        InventoryState::Reserved
                    } else {
                        InventoryState::Applied
                    },
                    execution_epoch: serde_json::from_value(row["execution_epoch"].clone())
                        .unwrap(),
                    manifest_sha256: if pending {
                        None
                    } else {
                        row["execution_evidence"]["runtime"]["manifest_sha256"]
                            .as_str()
                            .map(str::to_owned)
                    },
                    resource_budget: if pending {
                        None
                    } else {
                        Some(expected_plan(row, platform).unwrap())
                    },
                }
            })
            .collect();
        tx.commit().await.unwrap();
        Attestation {
            version: 1,
            challenge_id: serde_json::from_value(challenge["challenge_id"].clone()).unwrap(),
            executor_host_binding: binding.clone(),
            observed_capacity: Amount {
                cpu_millis: challenge["capacity"]["cpu_millis"].as_u64().unwrap(),
                memory_mib: challenge["capacity"]["memory_mib"].as_u64().unwrap(),
            },
            inventory_complete: true,
            unknown_workloads: false,
            allocations,
        }
    }
    struct Fixture {
        admin: sqlx::PgPool,
        pool: sqlx::PgPool,
        schema: String,
        binding: HostBinding,
        policy: PolicyRevision,
    }
    impl Fixture {
        async fn new(cpu: u64, memory: u64) -> Self {
            let url = std::env::var("COWORK_TEST_DATABASE_URL")
                .expect("isolated PostgreSQL test database URL");
            let admin = sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await
                .unwrap();
            let schema = format!("cowork_host_capacity_{}", Uuid::new_v4().simple());
            sqlx::query(&format!("CREATE SCHEMA {schema}"))
                .execute(&admin)
                .await
                .unwrap();
            let search_path = schema.clone();
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(12)
                .after_connect(move |connection, _| {
                    let path = search_path.clone();
                    Box::pin(async move {
                        sqlx::query("SELECT set_config('search_path',$1,false)")
                            .bind(path)
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
                CREATE TABLE management_organisations(id uuid PRIMARY KEY,policy jsonb NOT NULL,archived_at TIMESTAMPTZ);
                CREATE TABLE management_executor_hosts(id uuid PRIMARY KEY,trust_domain uuid NOT NULL,daemon_id text NOT NULL,
                    operator_state_id uuid NOT NULL,enabled boolean NOT NULL DEFAULT true,revision bigint NOT NULL DEFAULT 1,
                    capacity jsonb,UNIQUE(id,trust_domain));
                CREATE TABLE management_instances(id uuid PRIMARY KEY,organisation_id uuid NOT NULL,executor_host_id uuid,
                    policy jsonb NOT NULL,execution_epoch uuid NOT NULL,state text NOT NULL DEFAULT 'pending',execution_evidence jsonb);
                CREATE TABLE management_operations(id uuid PRIMARY KEY,instance_id uuid NOT NULL,organisation_id uuid NOT NULL,
                    action text NOT NULL,state text NOT NULL DEFAULT 'running',lease_token uuid,executor_host_binding jsonb);
                CREATE TABLE management_execution_reservations(id uuid PRIMARY KEY,instance_id uuid NOT NULL,state text NOT NULL DEFAULT 'held');
            "#).execute(&pool).await.unwrap();
            sqlx::raw_sql(include_str!(
                "../../migrations/0055_host_capacity_allocations.sql"
            ))
            .execute(&pool)
            .await
            .unwrap();
            let binding = HostBinding {
                version: 1,
                host_id: Uuid::new_v4(),
                trust_domain: Uuid::new_v4(),
                daemon_id: "d".repeat(64),
                operator_state_id: Uuid::new_v4(),
            };
            let policy = policy();
            sqlx::query("INSERT INTO management_organisations(id,policy) VALUES($1,$2)")
                .bind(binding.trust_domain)
                .bind(serde_json::to_value(&policy).unwrap())
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO management_executor_hosts(id,trust_domain,daemon_id,operator_state_id,capacity,service_plan) VALUES($1,$2,$3,$4,$5,$6)")
                .bind(binding.host_id).bind(binding.trust_domain).bind(&binding.daemon_id).bind(binding.operator_state_id)
                .bind(json!({"cpu_millis":cpu,"memory_mib":memory,"reserved_cpu_millis":1000,"reserved_memory_mib":1024}))
                .bind(service_plan()).execute(&pool).await.unwrap();
            qualify_existing(&pool, &binding, &policy).await;
            Self {
                admin,
                pool,
                schema,
                binding,
                policy,
            }
        }
        async fn close(self) {
            self.pool.close().await;
            let suffix = self.schema.strip_prefix("cowork_host_capacity_").unwrap();
            assert_eq!(suffix.len(), 32);
            assert!(Uuid::parse_str(suffix).is_ok());
            sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
                .execute(&self.admin)
                .await
                .unwrap();
            self.admin.close().await;
        }
        async fn create(&self) -> Uuid {
            let id = Uuid::new_v4();
            create(&self.pool, &self.binding, &self.policy, id)
                .await
                .unwrap();
            id
        }
        async fn applied(&self, id: Uuid) {
            sqlx::query("UPDATE management_instances SET state='ready',execution_evidence=jsonb_build_object('active_epoch',execution_epoch,'runtime',jsonb_build_object('version',1,'manifest_sha256',$2::text)) WHERE id=$1")
                .bind(id).bind("a".repeat(64)).execute(&self.pool).await.unwrap();
            qualify_existing(&self.pool, &self.binding, &self.policy).await;
        }
        async fn assert_held(&self, n: i64) {
            let row=sqlx::query("SELECT count(*) n,COALESCE(sum(cpu_millis),0)::bigint cpu,COALESCE(sum(memory_mib),0)::bigint memory FROM management_host_allocations WHERE state<>'released'")
                .fetch_one(&self.pool).await.unwrap();
            assert_eq!(row.get::<i64, _>("n"), n);
            assert_eq!(row.get::<i64, _>("cpu"), n * 2750);
            assert_eq!(row.get::<i64, _>("memory"), n * 1920);
        }
        async fn admitted(&self, id: Uuid) -> Result<(), ApiError> {
            let mut tx = context(&self.pool, self.binding.trust_domain).await;
            let epoch: Uuid =
                sqlx::query_scalar("SELECT execution_epoch FROM management_instances WHERE id=$1")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
            let result = require_admission(
                &mut tx,
                self.binding.trust_domain,
                self.binding.host_id,
                id,
                epoch,
                &self.policy,
            )
            .await;
            tx.rollback().await.unwrap();
            result
        }
    }
    async fn create(
        pool: &sqlx::PgPool,
        binding: &HostBinding,
        policy: &PolicyRevision,
        id: Uuid,
    ) -> Result<(), ApiError> {
        let mut tx = context(pool, binding.trust_domain).await;
        sqlx::query("INSERT INTO management_instances(id,organisation_id,executor_host_id,policy,execution_epoch) VALUES($1,$2,$3,$4,$5)")
            .bind(id).bind(binding.trust_domain).bind(binding.host_id).bind(serde_json::to_value(policy)?).bind(Uuid::new_v4()).execute(&mut *tx).await?;
        let result =
            reserve_instance(&mut tx, binding.trust_domain, binding.host_id, id, policy).await;
        match result {
            Ok(()) => tx.commit().await?,
            Err(error) => {
                tx.rollback().await?;
                return Err(error);
            }
        }
        Ok(())
    }

    #[test]
    fn static_plan_counts_services_once_and_never_multiplies_by_run_slots() {
        let services: ServicePlan = serde_json::from_value(service_plan()).unwrap();
        services.validate().unwrap();
        let first = ResourcePlan::new(Uuid::new_v4(), services.clone(), Pool::policy(&policy()));
        assert_eq!(
            first.total,
            Amount {
                cpu_millis: 2750,
                memory_mib: 1920
            }
        );
        let mut many = policy();
        many.limits.concurrent_runs = 1000;
        assert_eq!(
            ResourcePlan::new(first.instance_id, services, Pool::policy(&many)).total,
            first.total
        );
        assert!(serde_json::from_value::<ServicePlan>(json!({"api":service_plan()["api"],"worker":service_plan()["worker"],"runner":service_plan()["runner"]})).is_err());
        let mut bad = service_plan();
        bad["api"]["cpu_millis"] = json!(true);
        assert!(serde_json::from_value::<ServicePlan>(bad).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_forty_instance_creations_enforce_real_host_aggregate_and_no_ghost_rows() {
        for (cpu, memory) in [(12_000, 100_000), (100_000, 8704)] {
            let fixture = Fixture::new(cpu, memory).await;
            let pool = fixture.pool.clone();
            let binding = fixture.binding.clone();
            let policy = fixture.policy.clone();
            let mut body = tokio::spawn(async move {
                let barrier = Arc::new(Barrier::new(40));
                let mut tasks = JoinSet::new();
                for _ in 0..40 {
                    let (pool, binding, policy, barrier) = (
                        pool.clone(),
                        binding.clone(),
                        policy.clone(),
                        barrier.clone(),
                    );
                    tasks.spawn(async move {
                        barrier.wait().await;
                        create(&pool, &binding, &policy, Uuid::new_v4()).await
                    });
                }
                let mut accepted = 0;
                let mut refused = 0;
                while let Some(result) = tasks.join_next().await {
                    match result.unwrap() {
                        Ok(()) => accepted += 1,
                        Err(ApiError::Quota(code)) => {
                            assert_eq!(code, "host_capacity_exhausted");
                            refused += 1;
                        }
                        other => panic!("unexpected creation: {other:?}"),
                    }
                }
                assert_eq!((accepted, refused), (4, 36));
                let instances: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM management_instances")
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                assert_eq!(instances, 4);
                let row=sqlx::query("SELECT count(*) n,sum(cpu_millis)::bigint cpu,sum(memory_mib)::bigint memory FROM management_host_allocations WHERE state<>'released'").fetch_one(&pool).await.unwrap();
                assert_eq!(row.get::<i64, _>("n"), 4);
                assert_eq!(row.get::<i64, _>("cpu"), 11_000);
                assert_eq!(row.get::<i64, _>("memory"), 7680);
                sqlx::query("UPDATE management_instances SET state=CASE WHEN id=(SELECT min(id::text)::uuid FROM management_instances) THEN 'deleting' ELSE 'stopped' END").execute(&pool).await.unwrap();
                assert!(matches!(
                    create(&pool, &binding, &policy, Uuid::new_v4()).await,
                    Err(ApiError::Quota(_))
                ));
            });
            let result = tokio::time::timeout(std::time::Duration::from_secs(45), &mut body).await;
            if result.is_err() {
                body.abort();
                let _ = body.await;
            }
            fixture.close().await;
            result.unwrap().unwrap();
        }
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_exact_inventory_revision_epoch_manifest_and_db_deadline_are_required() {
        let fixture = Fixture::new(100_000, 100_000).await;
        let body_pool = fixture.pool.clone();
        let binding = fixture.binding.clone();
        let policy = fixture.policy.clone();
        let body = tokio::spawn(async move {
            let id = Uuid::new_v4();
            let before_creation = report(&body_pool, &binding, &policy).await;
            create(&body_pool, &binding, &policy, id).await.unwrap();
            let mut tx = context(&body_pool, binding.trust_domain).await;
            assert!(matches!(
                attest_tx(&mut tx, &before_creation, &policy).await,
                Err(ApiError::Conflict(_))
            ));
            tx.rollback().await.unwrap();
            sqlx::query("UPDATE management_instances SET state='ready',execution_evidence=jsonb_build_object('active_epoch',execution_epoch,'runtime',jsonb_build_object('version',1,'manifest_sha256',$2::text)) WHERE id=$1")
                .bind(id).bind("a".repeat(64)).execute(&body_pool).await.unwrap();
            qualify_existing(&body_pool, &binding, &policy).await;
            for mutation in [
                "omit",
                "unknown",
                "incomplete",
                "epoch",
                "manifest",
                "plan",
                "capacity",
            ] {
                let mut request = report(&body_pool, &binding, &policy).await;
                match mutation {
                    "omit" => request.allocations.clear(),
                    "unknown" => request.unknown_workloads = true,
                    "incomplete" => request.inventory_complete = false,
                    "epoch" => request.allocations[0].execution_epoch = Uuid::new_v4(),
                    "manifest" => request.allocations[0].manifest_sha256 = Some("b".repeat(64)),
                    "plan" => {
                        request.allocations[0]
                            .resource_budget
                            .as_mut()
                            .unwrap()
                            .services
                            .worker
                            .cpu_millis += 1
                    }
                    "capacity" => request.observed_capacity.memory_mib = 1,
                    _ => unreachable!(),
                }
                let mut tx = context(&body_pool, binding.trust_domain).await;
                let decision = attest_tx(&mut tx, &request, &policy).await.unwrap();
                assert_eq!(decision["qualified"], false);
                tx.commit().await.unwrap();
                let mut tx = context(&body_pool, binding.trust_domain).await;
                let epoch: Uuid = sqlx::query_scalar(
                    "SELECT execution_epoch FROM management_instances WHERE id=$1",
                )
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
                assert!(matches!(
                    require_admission(
                        &mut tx,
                        binding.trust_domain,
                        binding.host_id,
                        id,
                        epoch,
                        &policy
                    )
                    .await,
                    Err(ApiError::Unavailable(_))
                ));
                tx.rollback().await.unwrap();
                qualify_existing(&body_pool, &binding, &policy).await;
            }
            let request = report(&body_pool, &binding, &policy).await;
            let challenge_started: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
                "SELECT created_at FROM management_host_capacity_challenges WHERE id=$1",
            )
            .bind(request.challenge_id)
            .fetch_one(&body_pool)
            .await
            .unwrap();
            let mut tx = context(&body_pool, binding.trust_domain).await;
            attest_tx(&mut tx, &request, &policy).await.unwrap();
            tx.commit().await.unwrap();
            let expires: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
                "SELECT expires_at FROM management_host_capacity_proofs WHERE host_id=$1",
            )
            .bind(binding.host_id)
            .fetch_one(&body_pool)
            .await
            .unwrap();
            assert_eq!(expires - challenge_started, chrono::Duration::seconds(120));
            let mut tx = context(&body_pool, binding.trust_domain).await;
            assert!(matches!(
                attest_tx(&mut tx, &request, &policy).await,
                Err(ApiError::Conflict(_))
            ));
            tx.rollback().await.unwrap();
            let request = report(&body_pool, &binding, &policy).await;
            sqlx::query("UPDATE management_host_capacity_challenges SET created_at=clock_timestamp()-interval '2 minutes',expires_at=clock_timestamp()-interval '1 second' WHERE id=$1")
                .bind(request.challenge_id).execute(&body_pool).await.unwrap();
            let mut tx = context(&body_pool, binding.trust_domain).await;
            assert!(matches!(
                attest_tx(&mut tx, &request, &policy).await,
                Err(ApiError::Conflict(_))
            ));
            tx.rollback().await.unwrap();
            sqlx::query("UPDATE management_host_capacity_proofs SET expires_at=clock_timestamp()-interval '1 second' WHERE host_id=$1").bind(binding.host_id).execute(&body_pool).await.unwrap();
            let mut tx = context(&body_pool, binding.trust_domain).await;
            let epoch: Uuid =
                sqlx::query_scalar("SELECT execution_epoch FROM management_instances WHERE id=$1")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
            assert!(matches!(
                require_admission(
                    &mut tx,
                    binding.trust_domain,
                    binding.host_id,
                    id,
                    epoch,
                    &policy
                )
                .await,
                Err(ApiError::Unavailable(_))
            ));
            tx.rollback().await.unwrap();
            assert!(matches!(
                create(&body_pool, &binding, &policy, Uuid::new_v4()).await,
                Err(ApiError::Unavailable(_))
            ));
            let held: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM management_host_allocations WHERE state<>'released'",
            )
            .fetch_one(&body_pool)
            .await
            .unwrap();
            assert_eq!(held, 1);
        });
        let result = body.await;
        fixture.close().await;
        result.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_policy_shrink_host_toggle_and_exact_deletion_never_release_uncertain_capacity(
    ) {
        let fixture = Fixture::new(100_000, 100_000).await;
        let first = fixture.create().await;
        fixture.applied(first).await;
        let second = fixture.create().await;
        fixture.applied(second).await;
        fixture.assert_held(2).await;
        fixture.admitted(first).await.unwrap();
        let pool = fixture.pool.clone();
        let binding = fixture.binding.clone();
        let policy = fixture.policy.clone();
        let body = tokio::spawn(async move {
            let old_request = report(&pool, &binding, &policy).await;
            let mut tx = context(&pool, binding.trust_domain).await;
            sqlx::query("UPDATE management_executor_hosts SET enabled=false,revision=revision+1 WHERE id=$1").bind(binding.host_id).execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
            assert!(matches!(
                create(&pool, &binding, &policy, Uuid::new_v4()).await,
                Err(ApiError::Unavailable(_))
            ));
            let mut tx = context(&pool, binding.trust_domain).await;
            sqlx::query(
                "UPDATE management_executor_hosts SET enabled=true,revision=revision+1 WHERE id=$1",
            )
            .bind(binding.host_id)
            .execute(&mut *tx)
            .await
            .unwrap();
            tx.commit().await.unwrap();
            let mut tx = context(&pool, binding.trust_domain).await;
            assert!(matches!(
                attest_tx(&mut tx, &old_request, &policy).await,
                Err(ApiError::Conflict(_))
            ));
            tx.rollback().await.unwrap();
            assert!(matches!(
                create(&pool, &binding, &policy, Uuid::new_v4()).await,
                Err(ApiError::Unavailable(_))
            ));
            qualify_existing(&pool, &binding, &policy).await;
            let before = report(&pool, &binding, &policy).await;
            let mut smaller = policy.clone();
            smaller.limits.cpu_millis = 100;
            smaller.limits.memory_mib = 128;
            smaller.limits.concurrent_runs = 1;
            let mut tx = context(&pool, binding.trust_domain).await;
            sqlx::query("UPDATE management_instances SET policy=$2 WHERE id=$1")
                .bind(first)
                .bind(serde_json::to_value(&smaller).unwrap())
                .execute(&mut *tx)
                .await
                .unwrap();
            invalidate_policy(&mut tx, first).await.unwrap();
            tx.commit().await.unwrap();
            let mut tx = context(&pool, binding.trust_domain).await;
            assert!(matches!(
                attest_tx(&mut tx, &before, &policy).await,
                Err(ApiError::Conflict(_))
            ));
            tx.rollback().await.unwrap();
            let mut unchanged_manifest = report(&pool, &binding, &policy).await;
            let first_report = unchanged_manifest
                .allocations
                .iter_mut()
                .find(|v| v.instance_id == first)
                .unwrap();
            first_report.resource_budget = Some(ResourcePlan::new(
                first,
                serde_json::from_value(service_plan()).unwrap(),
                Pool::policy(&policy),
            ));
            let mut tx = context(&pool, binding.trust_domain).await;
            let decision = attest_tx(&mut tx, &unchanged_manifest, &policy)
                .await
                .unwrap();
            assert_eq!(decision["qualified"], false);
            assert_eq!(decision["reason"], "host_allocation_plan_mismatch");
            tx.commit().await.unwrap();
            // Simulate the trusted operator applying the smaller manifest. The
            // frozen charge remains the original maximum even after fresh proof.
            sqlx::query("UPDATE management_instances SET execution_evidence=jsonb_build_object('active_epoch',execution_epoch,'runtime',jsonb_build_object('version',1,'manifest_sha256',$2::text)) WHERE id=$1")
                .bind(first).bind("c".repeat(64)).execute(&pool).await.unwrap();
            qualify_existing(&pool, &binding, &policy).await;
            let mut tx = context(&pool, binding.trust_domain).await;
            let epoch: Uuid =
                sqlx::query_scalar("SELECT execution_epoch FROM management_instances WHERE id=$1")
                    .bind(first)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
            require_admission(
                &mut tx,
                binding.trust_domain,
                binding.host_id,
                first,
                epoch,
                &smaller,
            )
            .await
            .unwrap();
            tx.rollback().await.unwrap();
            let maxima=sqlx::query("SELECT sum(cpu_millis)::bigint cpu,sum(memory_mib)::bigint memory FROM management_host_allocations WHERE state<>'released'").fetch_one(&pool).await.unwrap();
            assert_eq!(maxima.get::<i64, _>("cpu"), 5500);
            assert_eq!(maxima.get::<i64, _>("memory"), 3840);
            sqlx::query(
                "INSERT INTO management_execution_reservations(id,instance_id) VALUES($1,$2)",
            )
            .bind(Uuid::new_v4())
            .bind(first)
            .execute(&pool)
            .await
            .unwrap();
            for instance in [first, second] {
                let operation = Uuid::new_v4();
                let lease = Uuid::new_v4();
                let mut tx = context(&pool, binding.trust_domain).await;
                sqlx::query("INSERT INTO management_operations(id,instance_id,organisation_id,action,lease_token,executor_host_binding) VALUES($1,$2,$3,'delete',$4,$5)")
                    .bind(operation).bind(instance).bind(binding.trust_domain).bind(lease).bind(json!(binding)).execute(&mut *tx).await.unwrap();
                let mut op: Value = sqlx::query_scalar(
                    "SELECT to_jsonb(o) FROM management_operations o WHERE id=$1",
                )
                .bind(operation)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
                let instance_row: Value = sqlx::query_scalar(
                    "SELECT to_jsonb(i) FROM management_instances i WHERE id=$1",
                )
                .bind(instance)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
                prepare_operation(&mut tx, &mut op, &instance_row)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
                let proof = json!({"receipt_id":operation,"instance_id":instance,"deletion_verified":true,
                    "deletion_inventory_sha256":"e".repeat(64),"host_allocation":op["host_allocation"]["binding"]});
                let mut wrong = proof.clone();
                wrong["host_allocation"]["allocation_revision"] = json!(999);
                let mut tx = context(&pool, binding.trust_domain).await;
                let row = sqlx::query(
                    "SELECT * FROM management_operations WHERE id=$1 AND lease_token=$2 FOR UPDATE",
                )
                .bind(operation)
                .bind(lease)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
                assert!(matches!(
                    complete(&mut tx, &row, true, &wrong).await,
                    Err(ApiError::Unprocessable(_))
                ));
                tx.rollback().await.unwrap();
                let mut tx = context(&pool, binding.trust_domain).await;
                let row = sqlx::query(
                    "SELECT * FROM management_operations WHERE id=$1 AND lease_token=$2 FOR UPDATE",
                )
                .bind(operation)
                .bind(lease)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
                if instance == first {
                    assert!(matches!(
                        complete(&mut tx, &row, true, &proof).await,
                        Err(ApiError::Conflict(_))
                    ));
                    tx.rollback().await.unwrap();
                } else {
                    complete(&mut tx, &row, true, &proof).await.unwrap();
                    sqlx::query("UPDATE management_instances SET state='deleted' WHERE id=$1")
                        .bind(instance)
                        .execute(&mut *tx)
                        .await
                        .unwrap();
                    tx.commit().await.unwrap();
                }
            }
            let held=sqlx::query("SELECT count(*) n,sum(cpu_millis)::bigint cpu,sum(memory_mib)::bigint memory FROM management_host_allocations WHERE state<>'released'").fetch_one(&pool).await.unwrap();
            assert_eq!(held.get::<i64, _>("n"), 1);
            assert_eq!(held.get::<i64, _>("cpu"), 2750);
            assert_eq!(held.get::<i64, _>("memory"), 1920);
            let still_held: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM management_execution_reservations WHERE state='held'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(still_held, 1);
        });
        let result = body.await;
        fixture.close().await;
        result.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a disposable PostgreSQL database via TEST_DATABASE_URL"]
    async fn postgres_frozen_operation_envelopes_and_all_action_receipts_preserve_exact_bindings() {
        let fixture = Fixture::new(100_000, 100_000).await;
        let instance = fixture.create().await;
        fixture.applied(instance).await;
        let pool = fixture.pool.clone();
        let binding = fixture.binding.clone();
        let policy = fixture.policy.clone();
        let body = tokio::spawn(async move {
            for action in [
                "provision",
                "start",
                "stop",
                "suspend",
                "resume",
                "backup",
                "restore",
                "upgrade",
                "rollback",
                "enroll_execution",
                "delete",
            ] {
                qualify_existing(&pool, &binding, &policy).await;
                let operation = Uuid::new_v4();
                let lease = Uuid::new_v4();
                let mut tx = context(&pool, binding.trust_domain).await;
                sqlx::query("INSERT INTO management_operations(id,instance_id,organisation_id,action,lease_token,executor_host_binding) VALUES($1,$2,$3,$4,$5,$6)")
                    .bind(operation).bind(instance).bind(binding.trust_domain).bind(action).bind(lease).bind(json!(binding))
                    .execute(&mut *tx).await.unwrap();
                let mut claimed: Value = sqlx::query_scalar(
                    "SELECT to_jsonb(o) FROM management_operations o WHERE id=$1",
                )
                .bind(operation)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
                let instance_row: Value = sqlx::query_scalar(
                    "SELECT to_jsonb(i) FROM management_instances i WHERE id=$1",
                )
                .bind(instance)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
                prepare_operation(&mut tx, &mut claimed, &instance_row)
                    .await
                    .unwrap();
                let original = claimed["host_allocation"].clone();
                let mut pending: Value = sqlx::query_scalar("SELECT to_jsonb(o)||jsonb_build_object('instance',to_jsonb(i)) FROM management_operations o JOIN management_instances i ON i.id=o.instance_id WHERE o.id=$1 FOR UPDATE OF o")
                    .bind(operation).fetch_one(&mut *tx).await.unwrap();
                assert!(pending["host_allocation"].is_null());
                attach_frozen_operation(&mut tx, &mut pending)
                    .await
                    .unwrap();
                assert_eq!(
                    pending["host_allocation"], original,
                    "{action}: pending must preserve the exact claim envelope"
                );
                assert_eq!(pending["host_allocation_binding"], original["binding"]);
                tx.commit().await.unwrap();

                let receipt = json!({"receipt_id":operation,"instance_id":instance,
                    "host_allocation":original["binding"],"deletion_verified":true,
                    "deletion_inventory_sha256":"e".repeat(64)});
                let mut missing = receipt.clone();
                missing.as_object_mut().unwrap().remove("host_allocation");
                let mut modified = receipt.clone();
                modified["host_allocation"]["allocation_revision"] = json!(999_999);
                for succeeded in [false, true] {
                    for refused in [&missing, &modified] {
                        let mut tx = context(&pool, binding.trust_domain).await;
                        let row = sqlx::query("SELECT * FROM management_operations WHERE id=$1 AND lease_token=$2 FOR UPDATE")
                            .bind(operation).bind(lease).fetch_one(&mut *tx).await.unwrap();
                        let before: Value = sqlx::query_scalar("SELECT to_jsonb(a) FROM management_host_allocations a WHERE instance_id=$1")
                            .bind(instance).fetch_one(&mut *tx).await.unwrap();
                        assert!(
                            matches!(
                                complete(&mut tx, &row, succeeded, refused).await,
                                Err(ApiError::Unprocessable(_))
                            ),
                            "{action}: missing or altered receipt binding must be refused"
                        );
                        let after: Value = sqlx::query_scalar("SELECT to_jsonb(a) FROM management_host_allocations a WHERE instance_id=$1")
                            .bind(instance).fetch_one(&mut *tx).await.unwrap();
                        assert_eq!(
                            before, after,
                            "{action}: refusal must precede invalidation or release"
                        );
                        // Commit after the Rust validation error to prove there
                        // were no earlier allocation mutations to roll back.
                        tx.commit().await.unwrap();
                    }
                }
                let mut tx = context(&pool, binding.trust_domain).await;
                let row = sqlx::query(
                    "SELECT * FROM management_operations WHERE id=$1 AND lease_token=$2 FOR UPDATE",
                )
                .bind(operation)
                .bind(lease)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
                complete(&mut tx, &row, true, &receipt).await.unwrap();
                let allocation: Value = sqlx::query_scalar(
                    "SELECT to_jsonb(a) FROM management_host_allocations a WHERE instance_id=$1",
                )
                .bind(instance)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
                assert_eq!(
                    allocation["plan"], original["plan"],
                    "retained maxima never change during completion"
                );
                if action == "delete" {
                    assert_eq!(allocation["state"], "released");
                    // Recovery can still read the exact original plan/binding,
                    // even when the ledger is released and current epoch differs.
                    sqlx::query("UPDATE management_instances SET state='deleted',execution_epoch=$2 WHERE id=$1")
                        .bind(instance).bind(Uuid::new_v4()).execute(&mut *tx).await.unwrap();
                    let mut recovered: Value = sqlx::query_scalar("SELECT to_jsonb(o)||jsonb_build_object('instance',to_jsonb(i)) FROM management_operations o JOIN management_instances i ON i.id=o.instance_id WHERE o.id=$1")
                        .bind(operation).fetch_one(&mut *tx).await.unwrap();
                    attach_frozen_operation(&mut tx, &mut recovered)
                        .await
                        .unwrap();
                    assert_eq!(recovered["host_allocation"], original);
                } else if action == "backup" {
                    assert_eq!(allocation["state"], "verified");
                    assert_eq!(
                        allocation["revision"],
                        original["binding"]["allocation_revision"]
                    );
                } else {
                    assert_eq!(allocation["state"], "unknown");
                    assert_eq!(
                        allocation["revision"].as_i64().unwrap(),
                        original["binding"]["allocation_revision"].as_i64().unwrap() + 1
                    );
                }
                tx.commit().await.unwrap();
            }
            // Operations without a bound allocation keep legacy semantics.
            let operation = Uuid::new_v4();
            let mut tx = context(&pool, binding.trust_domain).await;
            sqlx::query("INSERT INTO management_operations(id,instance_id,organisation_id,action) VALUES($1,$2,$3,'stop')")
                .bind(operation).bind(instance).bind(binding.trust_domain).execute(&mut *tx).await.unwrap();
            let row = sqlx::query("SELECT * FROM management_operations WHERE id=$1 FOR UPDATE")
                .bind(operation)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            complete(&mut tx, &row, true, &json!({})).await.unwrap();
            tx.commit().await.unwrap();
        });
        let result = body.await;
        fixture.close().await;
        result.unwrap();
    }
}
