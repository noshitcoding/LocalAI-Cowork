//! Operator-owned catalog; public callers can only choose a known immutable release.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseManifest {
    version: u32,
    id: String,
    name: String,
    images: BTreeMap<String, String>,
    supported_sources: Vec<String>,
}
impl ReleaseManifest {
    fn validate(&self) -> Result<(), ApiError> {
        let expected: BTreeSet<_> = [
            "server",
            "runner",
            "sandbox_core",
            "sandbox_gui",
            "sandbox_crew",
            "egress_proxy",
        ]
        .into_iter()
        .collect();
        if self.version != 1
            || !digest(&self.id)
            || self.name.is_empty()
            || self.name.chars().count() > 120
            || self.name.chars().any(char::is_control)
            || self
                .images
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != expected
            || self.supported_sources.is_empty()
            || self.supported_sources.len() > 256
            || self.supported_sources.iter().any(|s| !digest(s))
            || self.supported_sources.iter().collect::<BTreeSet<_>>().len()
                != self.supported_sources.len()
        {
            return Err(invalid("invalid release manifest"));
        }
        for image in self.images.values() {
            let (repository, hash) = image
                .split_once("@sha256:")
                .ok_or_else(|| invalid("release images require immutable digests"))?;
            if repository.is_empty()
                || !repository.as_bytes()[0].is_ascii_alphanumeric()
                || !repository
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._:/-".contains(&b))
                || !digest(hash)
            {
                return Err(invalid("invalid release image"));
            }
        }
        if hex::encode(Sha256::digest(serde_json::to_vec(&self.images)?)) != self.id {
            return Err(invalid("release image digest mismatch"));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    instance_id: Uuid,
    organisation_id: Uuid,
    release: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CatalogSync {
    releases: Vec<ReleaseManifest>,
    instances: Vec<Inventory>,
    executor_host_binding: Option<executor_hosts::HostBinding>,
}

pub(super) async fn sync_releases(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CatalogSync>,
) -> Result<Json<Value>, ApiError> {
    let operator = executor_hosts::authenticate(&state, &headers).await?;
    operator.verify_claim(request.executor_host_binding.as_ref())?;
    if !operator.enabled {
        return Err(denied());
    }
    if request.releases.len() > 256
        || request.instances.len() > 4096
        || request
            .releases
            .iter()
            .map(|r| &r.id)
            .collect::<BTreeSet<_>>()
            .len()
            != request.releases.len()
        || request
            .instances
            .iter()
            .map(|i| i.instance_id)
            .collect::<BTreeSet<_>>()
            .len()
            != request.instances.len()
    {
        return Err(invalid("invalid release inventory"));
    }
    for release in &request.releases {
        release.validate()?;
    }
    if request.instances.iter().any(|i| !digest(&i.release)) {
        return Err(invalid("invalid installed release"));
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    if !operator.current(&mut tx).await? {
        return Err(denied());
    }
    // Serialize full catalog snapshots. No request supplies filesystem paths or commands.
    sqlx::query("SELECT pg_advisory_xact_lock(7182483621)")
        .execute(&mut *tx)
        .await?;
    if let Some(host) = operator.host_id() {
        sqlx::query("UPDATE management_executor_host_releases SET active=FALSE WHERE host_id=$1")
            .bind(host)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("UPDATE management_releases SET active=FALSE")
            .execute(&mut *tx)
            .await?;
    }
    for release in &request.releases {
        let manifest = serde_json::to_value(release)?;
        let previous: Option<Value> =
            sqlx::query_scalar("SELECT manifest FROM management_releases WHERE id=$1 FOR UPDATE")
                .bind(&release.id)
                .fetch_optional(&mut *tx)
                .await?;
        if previous.is_some_and(|v| v != manifest) {
            return Err(ApiError::Conflict("release manifest is immutable".into()));
        }
        // Manifest truth is shared; availability is owned by exactly one operator.
        sqlx::query("INSERT INTO management_releases(id,manifest,active) VALUES($1,$2,$3) ON CONFLICT(id) DO UPDATE SET active=CASE WHEN $3 THEN TRUE ELSE management_releases.active END,refreshed_at=CASE WHEN $3 THEN now() ELSE management_releases.refreshed_at END")
            .bind(&release.id).bind(manifest).bind(operator.host_id().is_none()).execute(&mut *tx).await?;
        if let Some(host) = operator.host_id() {
            sqlx::query("INSERT INTO management_executor_host_releases(host_id,release_id) VALUES($1,$2) ON CONFLICT(host_id,release_id) DO UPDATE SET active=TRUE,refreshed_at=now()")
                .bind(host).bind(&release.id).execute(&mut *tx).await?;
        }
    }
    for installed in request.instances {
        let found: Option<Uuid> = sqlx::query_scalar("SELECT id FROM management_instances i WHERE id=$1 AND organisation_id=$2 AND (backend='compose' OR backend='kubernetes' AND EXISTS(SELECT 1 FROM management_backend_profiles p WHERE p.instance_id=i.id AND p.host_id=i.executor_host_id AND p.enabled)) AND state<>'deleted' AND executor_host_id IS NOT DISTINCT FROM $3 FOR UPDATE")
            .bind(installed.instance_id).bind(installed.organisation_id).bind(operator.host_id()).fetch_optional(&mut *tx).await?;
        if found.is_none() {
            return Err(denied());
        }
        // Never replace the frozen source while an operation is pending or ambiguous.
        sqlx::query("UPDATE management_instances i SET current_release=$3,revision=revision+1 WHERE id=$1 AND organisation_id=$2 AND current_release IS DISTINCT FROM $3 AND NOT EXISTS(SELECT 1 FROM management_operations o WHERE o.instance_id=i.id AND o.state IN ('queued','running','reconciling'))")
            .bind(installed.instance_id).bind(installed.organisation_id).bind(installed.release).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"synchronized":true})))
}

pub(super) async fn instance_releases(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<InstanceRelease>>, ApiError> {
    instance_access(&state, &principal, org, id, false).await?;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',r.id,'name',r.manifest->>'name','source_release',i.current_release,'qualified',EXISTS(SELECT 1 FROM management_release_qualifications q JOIN management_operations o ON o.id=q.operation_id WHERE q.release_id=r.id AND q.source_release=i.current_release AND q.executor_host_id IS NOT DISTINCT FROM i.executor_host_id AND o.state='succeeded' AND o.backend_profile IS NOT DISTINCT FROM (SELECT to_jsonb(p)-'receipt' FROM management_backend_profiles p WHERE p.instance_id=i.id AND p.enabled))) FROM management_instances i JOIN management_releases r ON (r.manifest->'supported_sources') ? i.current_release WHERE i.id=$1 AND i.organisation_id=$2 AND ((i.executor_host_id IS NULL AND r.active AND r.refreshed_at>now()-interval '5 minutes') OR EXISTS(SELECT 1 FROM management_executor_host_releases hr JOIN management_executor_hosts h ON h.id=hr.host_id WHERE hr.host_id=i.executor_host_id AND hr.release_id=r.id AND hr.active AND h.enabled AND hr.refreshed_at>now()-interval '5 minutes')) AND r.id<>i.current_release ORDER BY r.manifest->>'name',r.id")
        .bind(id).bind(org).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()?,
    ))
}

pub(super) async fn selection_allowed(
    tx: &mut Transaction<'_, Postgres>,
    release: &str,
    source: &str,
    canary: bool,
    host: Option<Uuid>,
) -> Result<bool, ApiError> {
    selection_allowed_profiled(tx, release, source, canary, host, None).await
}
pub(super) async fn selection_allowed_profiled(
    tx: &mut Transaction<'_, Postgres>,
    release: &str,
    source: &str,
    canary: bool,
    host: Option<Uuid>,
    profile: Option<&Value>,
) -> Result<bool, ApiError> {
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_releases r WHERE r.id=$1 AND (($4::uuid IS NULL AND r.active AND r.refreshed_at>now()-interval '5 minutes') OR EXISTS(SELECT 1 FROM management_executor_host_releases hr JOIN management_executor_hosts h ON h.id=hr.host_id WHERE hr.host_id=$4 AND hr.release_id=r.id AND hr.active AND h.enabled AND hr.refreshed_at>now()-interval '5 minutes')) AND (r.manifest->'supported_sources') ? $2 AND r.id<>$2 AND ($3 OR EXISTS(SELECT 1 FROM management_release_qualifications q JOIN management_operations o ON o.id=q.operation_id WHERE q.release_id=r.id AND q.source_release=$2 AND q.executor_host_id IS NOT DISTINCT FROM $4 AND o.state='succeeded' AND o.backend_profile IS NOT DISTINCT FROM $5::jsonb)))")
        .bind(release).bind(source).bind(canary).bind(host).bind(profile).fetch_one(&mut **tx).await?;
    Ok(allowed)
}

pub(super) async fn validate_selection(
    tx: &mut Transaction<'_, Postgres>,
    release: &str,
    source: &str,
    canary: bool,
    host: Option<Uuid>,
) -> Result<(), ApiError> {
    validate_selection_profiled(tx, release, source, canary, host, None).await
}
pub(super) async fn validate_selection_profiled(
    tx: &mut Transaction<'_, Postgres>,
    release: &str,
    source: &str,
    canary: bool,
    host: Option<Uuid>,
    profile: Option<&Value>,
) -> Result<(), ApiError> {
    if !selection_allowed_profiled(tx, release, source, canary, host, profile).await? {
        return Err(invalid("release is unavailable, incompatible or requires a successful canary on this executor host"));
    }
    Ok(())
}

pub(super) async fn complete_update(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    instance: Uuid,
    arguments: &Value,
    result: &Value,
) -> Result<(), ApiError> {
    let source: Option<String> = sqlx::query_scalar(
        "SELECT current_release FROM management_instances WHERE id=$1 FOR UPDATE",
    )
    .bind(instance)
    .fetch_one(&mut **tx)
    .await?;
    let host = executor_hosts::instance_host(tx, instance).await?;
    let canary = arguments["qualification_only"].as_bool().unwrap_or(false);
    if result["receipt_id"] != json!(id)
        || result["instance_id"] != json!(instance)
        || result["source_release"] != arguments["source_release"]
        || result["source_release"].as_str() != source.as_deref()
        || result["release"] != arguments["release"]
        || result["qualification_only"] != json!(canary)
        || result["instance_state"] != "stopped"
        || (canary && (result["rollback_verified"] != true || !result["generation"].is_null()))
        || (!canary
            && (result["generation"] != json!(id) || !result["rollback_verified"].is_null()))
    {
        return Err(invalid(
            "update receipt does not match its requested transition",
        ));
    }
    let release = arguments["release"]
        .as_str()
        .ok_or_else(|| invalid("missing release"))?;
    if canary {
        sqlx::query("INSERT INTO management_release_qualifications(release_id,source_release,operation_id,executor_host_id) VALUES($1,$2,$3,$4) ON CONFLICT(release_id,source_release,executor_host_id) DO UPDATE SET operation_id=$3,created_at=now()")
            .bind(release).bind(source).bind(id).bind(host).execute(&mut **tx).await?;
    } else {
        sqlx::query("UPDATE management_instances SET current_release=$2 WHERE id=$1")
            .bind(instance)
            .bind(release)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}
