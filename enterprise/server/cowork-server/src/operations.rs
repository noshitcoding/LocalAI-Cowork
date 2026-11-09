use std::collections::BTreeMap;

use axum::{
    body::Body,
    extract::{Extension, State},
    http::{header, HeaderValue, Response, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use cowork_contracts::{API_VERSION, MIN_COMPATIBLE_SCHEMA_VERSION, SCHEMA_VERSION};
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use crate::{auth::Principal, db, error::ApiError, AppState};

#[derive(Debug, Clone, Serialize)]
pub struct OperationsSnapshot {
    schema_version: u16,
    generated_at: DateTime<Utc>,
    application: ApplicationMetrics,
    database: DatabaseMetrics,
    workload: WorkloadMetrics,
    storage: StorageMetrics,
    delivery: DeliveryMetrics,
    runtime: crate::runtime_status::RuntimeStatus,
}

#[derive(Debug, Clone, Serialize)]
struct ApplicationMetrics {
    build_version: &'static str,
    api_version: &'static str,
    minimum_compatible_schema_version: u16,
    database_migration_version: i64,
    object_store_configured: bool,
    runner_configured: bool,
    push_configured: bool,
    passkeys_configured: bool,
    oidc_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
struct DatabaseMetrics {
    users: i64,
    teams: i64,
    projects: i64,
    threads: i64,
    audit_events: i64,
}

#[derive(Debug, Clone, Serialize)]
struct WorkloadMetrics {
    runs_by_state: BTreeMap<String, i64>,
    schedules_enabled: i64,
    schedules_overdue: i64,
    approvals_waiting: i64,
    input_requests_waiting: i64,
    executors_registered: i64,
    executors_recently_seen: i64,
    active_support_grants: i64,
}

#[derive(Debug, Clone, Serialize)]
struct StorageMetrics {
    snapshots_by_state: BTreeMap<String, i64>,
    ready_chunk_plaintext_bytes: i64,
    ready_chunk_ciphertext_bytes: i64,
    unreferenced_chunks: i64,
    live_artifact_bytes: i64,
}

#[derive(Debug, Clone, Serialize)]
struct DeliveryMetrics {
    active_push_subscriptions: i64,
    pending_push_deliveries: i64,
    failed_push_deliveries: i64,
}

pub async fn metrics(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    headers: axum::http::HeaderMap,
) -> Result<Response<Body>, ApiError> {
    require_platform_admin(&state, principal.user_id).await?;
    let snapshot = collect_snapshot(&state).await?;
    if headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|item| item.trim().starts_with("text/plain"))
        })
    {
        Ok((
            [
                (
                    header::CONTENT_TYPE,
                    "text/plain; version=0.0.4; charset=utf-8",
                ),
                (header::CACHE_CONTROL, "no-store"),
            ],
            prometheus(&snapshot),
        )
            .into_response())
    } else {
        Ok(Json(snapshot).into_response())
    }
}

pub async fn support_bundle(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Response<Body>, ApiError> {
    require_platform_admin(&state, principal.user_id).await?;
    let snapshot = collect_snapshot(&state).await?;
    let generated_at = snapshot.generated_at;
    sqlx::query(
        "INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, 'operations.support_bundle.export', 'server', $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(principal.user_id)
    .bind(Uuid::nil())
    .bind(serde_json::json!({"generated_at": generated_at}))
    .execute(&state.pool)
    .await?;

    let body = serde_json::to_vec_pretty(&snapshot)?;
    let filename = format!(
        "open-cowork-support-{}.json",
        generated_at.format("%Y%m%dT%H%M%SZ")
    );
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .map_err(|error| ApiError::Internal(error.into()))?,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn require_platform_admin(state: &AppState, user_id: Uuid) -> Result<(), ApiError> {
    if db::user_is_platform_admin(&state.pool, user_id).await? {
        Ok(())
    } else {
        Err(ApiError::Unauthorized(
            "platform administrator role is required for server operations".to_owned(),
        ))
    }
}

async fn collect_snapshot(state: &AppState) -> Result<OperationsSnapshot, ApiError> {
    let runs_by_state = grouped_counts(
        &state.pool,
        "SELECT state, count(*) FROM runs GROUP BY state",
    )
    .await?;
    let snapshots_by_state = grouped_counts(
        &state.pool,
        "SELECT status, count(*) FROM snapshot_manifests GROUP BY status",
    )
    .await?;
    let recently_seen_seconds = state.lease_seconds.saturating_mul(2).max(60);

    Ok(OperationsSnapshot {
        schema_version: SCHEMA_VERSION,
        generated_at: Utc::now(),
        application: ApplicationMetrics {
            build_version: env!("CARGO_PKG_VERSION"),
            api_version: API_VERSION,
            minimum_compatible_schema_version: MIN_COMPATIBLE_SCHEMA_VERSION,
            database_migration_version: scalar(
                &state.pool,
                "SELECT COALESCE(max(version), 0) FROM _sqlx_migrations WHERE success",
            )
            .await?,
            object_store_configured: state.object_store.is_some(),
            runner_configured: state.runner.is_some(),
            push_configured: state.push.is_some(),
            passkeys_configured: state.webauthn.is_some(),
            oidc_configured: state.oidc.is_some(),
        },
        database: DatabaseMetrics {
            users: scalar(&state.pool, "SELECT count(*) FROM users WHERE deleted_at IS NULL").await?,
            teams: scalar(&state.pool, "SELECT count(*) FROM teams WHERE deleted_at IS NULL").await?,
            projects: scalar(&state.pool, "SELECT count(*) FROM projects WHERE deleted_at IS NULL").await?,
            threads: scalar(&state.pool, "SELECT count(*) FROM threads WHERE deleted_at IS NULL").await?,
            audit_events: scalar(&state.pool, "SELECT count(*) FROM audit_events").await?,
        },
        workload: WorkloadMetrics {
            runs_by_state,
            schedules_enabled: scalar(&state.pool, "SELECT count(*) FROM schedules WHERE enabled AND deleted_at IS NULL").await?,
            schedules_overdue: scalar(&state.pool, "SELECT count(*) FROM schedules WHERE enabled AND deleted_at IS NULL AND next_run_at <= now()").await?,
            approvals_waiting: scalar(&state.pool, "SELECT count(*) FROM approval_requests WHERE state = 'pending'").await?,
            input_requests_waiting: scalar(&state.pool, "SELECT count(*) FROM run_input_requests WHERE state = 'pending'").await?,
            executors_registered: scalar(&state.pool, "SELECT count(*) FROM executors").await?,
            executors_recently_seen: sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM executors WHERE last_seen_at > now() - ($1::double precision * interval '1 second')",
            )
            .bind(recently_seen_seconds as f64)
            .fetch_one(&state.pool)
            .await?,
            active_support_grants: scalar(&state.pool, "SELECT count(*) FROM support_grants WHERE revoked_at IS NULL AND expires_at > now()").await?,
        },
        storage: StorageMetrics {
            snapshots_by_state,
            ready_chunk_plaintext_bytes: scalar(&state.pool, "SELECT COALESCE(sum(plaintext_size), 0)::bigint FROM snapshot_chunks WHERE status = 'ready'").await?,
            ready_chunk_ciphertext_bytes: scalar(&state.pool, "SELECT COALESCE(sum(ciphertext_size), 0)::bigint FROM snapshot_chunks WHERE status = 'ready'").await?,
            unreferenced_chunks: scalar(&state.pool, "SELECT count(*) FROM snapshot_chunks WHERE ref_count = 0").await?,
            live_artifact_bytes: scalar(&state.pool, "SELECT COALESCE(sum(size_bytes), 0)::bigint FROM run_artifacts WHERE deleted_at IS NULL").await?,
        },
        delivery: DeliveryMetrics {
            active_push_subscriptions: scalar(&state.pool, "SELECT count(*) FROM push_subscriptions WHERE revoked_at IS NULL").await?,
            pending_push_deliveries: scalar(&state.pool, "SELECT count(*) FROM push_deliveries WHERE state IN ('queued', 'processing')").await?,
            failed_push_deliveries: scalar(&state.pool, "SELECT count(*) FROM push_deliveries WHERE state = 'failed'").await?,
        },
        runtime: crate::runtime_status::collect(state).await?,
    })
}

fn prometheus(snapshot: &OperationsSnapshot) -> String {
    use std::fmt::Write;
    let mut text = String::new();
    for (name, value) in [
        (
            "cowork_queue_oldest_age_seconds",
            snapshot.runtime.queue_oldest_age_seconds,
        ),
        (
            "cowork_expired_active_leases",
            snapshot.runtime.expired_active_leases,
        ),
        (
            "cowork_schedules_overdue",
            snapshot.workload.schedules_overdue,
        ),
        (
            "cowork_database_migration_version",
            snapshot.application.database_migration_version,
        ),
        (
            "cowork_central_admission_enabled",
            i64::from(snapshot.runtime.features.central_admission),
        ),
        (
            "cowork_chat_workspace_allocation_enabled",
            i64::from(snapshot.runtime.features.chat_workspace_allocation),
        ),
        (
            "cowork_event_listener_connected",
            i64::from(
                snapshot.runtime.event_delivery["listener_connected"]
                    .as_bool()
                    .unwrap_or(false),
            ),
        ),
        (
            "cowork_api_ownership_healthy",
            i64::from(
                snapshot.runtime.api_routing["ownership_healthy"]
                    .as_bool()
                    .unwrap_or(false),
            ),
        ),
    ] {
        let _ = writeln!(text, "# TYPE {name} gauge\n{name} {value}");
    }
    text.push_str("# TYPE cowork_runs gauge\n");
    for (state, count) in &snapshot.workload.runs_by_state {
        let _ = writeln!(
            text,
            "cowork_runs{{state={}}} {count}",
            serde_json::to_string(state).unwrap()
        );
    }
    for group in &snapshot.runtime.process_groups {
        let labels = format!(
            "mode={},build={},migration=\"{}\",lifecycle={}",
            serde_json::to_string(&group.mode).unwrap(),
            serde_json::to_string(&group.build_version).unwrap(),
            group.migration_version,
            serde_json::to_string(&group.lifecycle).unwrap()
        );
        for (name, value) in [
            ("cowork_processes", group.processes),
            ("cowork_fresh_processes", group.fresh_processes),
            ("cowork_active_runs", group.active_runs),
            (
                "cowork_scheduler_unhealthy_processes",
                group.scheduler_unhealthy_processes,
            ),
            (
                "cowork_maintenance_unhealthy_processes",
                group.maintenance_unhealthy_processes,
            ),
            (
                "cowork_dispatch_unhealthy_processes",
                group.dispatch_unhealthy_processes,
            ),
            (
                "cowork_process_pulse_age_seconds",
                group.last_pulse_age_seconds,
            ),
        ] {
            let _ = writeln!(text, "{name}{{{labels}}} {value}");
        }
        for (name, value) in [
            ("cowork_scheduler_age_seconds", group.scheduler_age_seconds),
            (
                "cowork_maintenance_age_seconds",
                group.maintenance_age_seconds,
            ),
            ("cowork_dispatch_age_seconds", group.dispatch_age_seconds),
        ] {
            if let Some(value) = value {
                let _ = writeln!(text, "{name}{{{labels}}} {value}");
            }
        }
    }
    text
}

async fn scalar(pool: &sqlx::PgPool, query: &str) -> Result<i64, ApiError> {
    Ok(sqlx::query_scalar::<_, i64>(query).fetch_one(pool).await?)
}

async fn grouped_counts(
    pool: &sqlx::PgPool,
    query: &str,
) -> Result<BTreeMap<String, i64>, ApiError> {
    let rows = sqlx::query(query).fetch_all(pool).await?;
    let mut values = BTreeMap::new();
    for row in rows {
        values.insert(row.try_get(0)?, row.try_get(1)?);
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_snapshot_contract_contains_only_aggregate_sections() {
        let snapshot = OperationsSnapshot {
            schema_version: 1,
            generated_at: Utc::now(),
            application: ApplicationMetrics {
                build_version: "test",
                api_version: "v1",
                minimum_compatible_schema_version: 1,
                database_migration_version: 19,
                object_store_configured: true,
                runner_configured: true,
                push_configured: false,
                passkeys_configured: false,
                oidc_configured: false,
            },
            database: DatabaseMetrics {
                users: 1,
                teams: 0,
                projects: 0,
                threads: 0,
                audit_events: 1,
            },
            workload: WorkloadMetrics {
                runs_by_state: BTreeMap::new(),
                schedules_enabled: 0,
                schedules_overdue: 0,
                approvals_waiting: 0,
                input_requests_waiting: 0,
                executors_registered: 0,
                executors_recently_seen: 0,
                active_support_grants: 0,
            },
            storage: StorageMetrics {
                snapshots_by_state: BTreeMap::new(),
                ready_chunk_plaintext_bytes: 0,
                ready_chunk_ciphertext_bytes: 0,
                unreferenced_chunks: 0,
                live_artifact_bytes: 0,
            },
            delivery: DeliveryMetrics {
                active_push_subscriptions: 0,
                pending_push_deliveries: 0,
                failed_push_deliveries: 0,
            },
            runtime: crate::runtime_status::RuntimeStatus {
                process_groups: Vec::new(),
                queue_oldest_age_seconds: 0,
                expired_active_leases: 0,
                features: crate::runtime_status::FeatureStatus {
                    process_role: "instance",
                    api_replica_policy: "single_connection_owner",
                    event_delivery: "postgres_notification_with_durable_replay",
                    chat_workspace_allocation: false,
                    central_admission: false,
                    deployment_qualification: "requires_image_bound_operator_evidence",
                },
                api_routing: serde_json::json!({"ownership_healthy":true}),
                event_delivery: serde_json::json!({"listener_connected":true}),
            },
        };
        let encoded = serde_json::to_string(&snapshot).unwrap();
        let export = prometheus(&snapshot);
        assert!(export.contains("cowork_queue_oldest_age_seconds 0\n"));
        assert!(!export.contains("cowork_scheduler_age_seconds"));
        for forbidden in [
            "email",
            "prompt",
            "content",
            "object_key",
            "token",
            "secret",
        ] {
            assert!(!encoded.contains(forbidden), "bundle exposed {forbidden}");
        }
    }
}
