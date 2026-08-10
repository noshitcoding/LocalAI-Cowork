use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use chrono::{Datelike, Utc};
use cowork_contracts::{
    CreateSupportGrantRequest, ProjectRole, QuotaLimitsRecord, QuotaScopeType, QuotaStatus,
    QuotaUsageRecord, SetQuotaLimitsRequest, SupportGrantRecord, SCHEMA_VERSION,
};
use serde_json::json;
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{auth::Principal, db, error::ApiError, organization, AppState};

pub async fn create_support_grant(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateSupportGrantRequest>,
) -> Result<(StatusCode, Json<SupportGrantRecord>), ApiError> {
    if request.project_id.is_some() == request.thread_id.is_some() {
        return Err(ApiError::Unprocessable(
            "a support grant must select exactly one project or thread".to_owned(),
        ));
    }
    if request.support_user_id == principal.user_id {
        return Err(ApiError::Unprocessable(
            "support access cannot be granted to the grantor".to_owned(),
        ));
    }
    let reason = request.reason.trim();
    if reason.is_empty() || reason.chars().count() > 1_000 {
        return Err(ApiError::Unprocessable(
            "support grant reason must contain 1 to 1000 characters".to_owned(),
        ));
    }
    let now = Utc::now();
    if request.expires_at <= now || request.expires_at > now + chrono::Duration::hours(24) {
        return Err(ApiError::Unprocessable(
            "support grants must expire within the next 24 hours".to_owned(),
        ));
    }
    let project_id = match (request.project_id, request.thread_id) {
        (Some(project_id), None) => project_id,
        (None, Some(thread_id)) => sqlx::query_scalar::<_, Uuid>(
            "SELECT project_id FROM threads WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(thread_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("thread {thread_id} was not found")))?,
        _ => unreachable!(),
    };
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    if !db::user_is_platform_admin(&state.pool, request.support_user_id).await? {
        return Err(ApiError::Unprocessable(
            "support access can only be granted to a platform administrator".to_owned(),
        ));
    }

    let id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        INSERT INTO support_grants
            (id, granted_by, support_user_id, project_id, thread_id, reason, created_at, expires_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(principal.user_id)
    .bind(request.support_user_id)
    .bind(request.project_id)
    .bind(request.thread_id)
    .bind(reason)
    .bind(now)
    .bind(request.expires_at)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, 'support_grant.create', 'support_grant', $3, $4)")
        .bind(Uuid::new_v4()).bind(principal.user_id).bind(id)
        .bind(json!({"support_user_id": request.support_user_id, "project_id": request.project_id, "thread_id": request.thread_id, "expires_at": request.expires_at}))
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row_to_support_grant(&row)?)))
}

pub async fn list_support_grants(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<SupportGrantRecord>>, ApiError> {
    let rows = if db::user_is_platform_admin(&state.pool, principal.user_id).await? {
        sqlx::query("SELECT * FROM support_grants ORDER BY created_at DESC LIMIT 500")
            .fetch_all(&state.pool)
            .await?
    } else {
        sqlx::query(
            r#"
            SELECT DISTINCT grant_row.* FROM support_grants grant_row
            LEFT JOIN threads thread ON thread.id = grant_row.thread_id
            JOIN projects project ON project.id = COALESCE(grant_row.project_id, thread.project_id)
            LEFT JOIN project_members pm ON pm.project_id = project.id AND pm.user_id = $1 AND pm.role = 'editor'
            LEFT JOIN team_members tm ON tm.team_id = project.team_id AND tm.user_id = $1 AND tm.role IN ('owner', 'admin')
            WHERE grant_row.granted_by = $1 OR project.owner_user_id = $1 OR pm.user_id IS NOT NULL OR tm.user_id IS NOT NULL
            ORDER BY grant_row.created_at DESC LIMIT 500
            "#,
        )
        .bind(principal.user_id)
        .fetch_all(&state.pool)
        .await?
    };
    Ok(Json(
        rows.iter()
            .map(row_to_support_grant)
            .collect::<Result<_, _>>()?,
    ))
}

pub async fn revoke_support_grant(
    State(state): State<AppState>,
    Path(grant_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT grant_row.*, COALESCE(grant_row.project_id, thread.project_id) AS scope_project_id
        FROM support_grants grant_row
        LEFT JOIN threads thread ON thread.id = grant_row.thread_id
        WHERE grant_row.id = $1
        "#,
    )
    .bind(grant_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("support grant {grant_id} was not found")))?;
    let support_user_id: Uuid = row.try_get("support_user_id")?;
    let granted_by: Uuid = row.try_get("granted_by")?;
    if principal.user_id != support_user_id && principal.user_id != granted_by {
        organization::ensure_project_role(
            &state.pool,
            principal.user_id,
            row.try_get("scope_project_id")?,
            ProjectRole::Editor,
        )
        .await?;
    }
    let mut tx = state.pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE support_grants SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL RETURNING id",
    )
    .bind(grant_id)
    .fetch_optional(&mut *tx)
    .await?;
    if updated.is_none() {
        return Err(ApiError::Conflict(
            "support grant is already revoked".to_owned(),
        ));
    }
    sqlx::query("INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, 'support_grant.revoke', 'support_grant', $3, $4)")
        .bind(Uuid::new_v4()).bind(principal.user_id).bind(grant_id)
        .bind(json!({"support_user_id": support_user_id})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_quota(
    State(state): State<AppState>,
    Path((scope_name, scope_id)): Path<(String, Uuid)>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<QuotaStatus>, ApiError> {
    let scope = parse_quota_scope(&scope_name)?;
    ensure_quota_view(&state.pool, principal.user_id, scope, scope_id).await?;
    Ok(Json(quota_status(&state.pool, scope, scope_id).await?))
}

pub async fn set_quota(
    State(state): State<AppState>,
    Path((scope_name, scope_id)): Path<(String, Uuid)>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<SetQuotaLimitsRequest>,
) -> Result<Json<QuotaStatus>, ApiError> {
    let scope = parse_quota_scope(&scope_name)?;
    match scope {
        QuotaScopeType::User => {
            if !db::user_is_platform_admin(&state.pool, principal.user_id).await? {
                return Err(ApiError::Unauthorized(
                    "platform administrator role is required to set user quotas".to_owned(),
                ));
            }
            let exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND deleted_at IS NULL)",
            )
            .bind(scope_id)
            .fetch_one(&state.pool)
            .await?;
            if !exists {
                return Err(ApiError::NotFound(format!("user {scope_id} was not found")));
            }
        }
        QuotaScopeType::Team => {
            organization::ensure_team_admin(&state.pool, principal.user_id, scope_id).await?;
        }
    }
    if request.monthly_cost_micros.is_some() && request.monthly_tokens.is_none() {
        return Err(ApiError::Unprocessable(
            "a monthly token fallback is required whenever a cost quota is configured".to_owned(),
        ));
    }
    let storage_bytes = optional_u64_to_i64(request.storage_bytes, "storage_bytes")?;
    let concurrent_runs = request.concurrent_runs.map(i64::from);
    let monthly_tokens = optional_u64_to_i64(request.monthly_tokens, "monthly_tokens")?;
    let monthly_cost = optional_u64_to_i64(request.monthly_cost_micros, "monthly_cost_micros")?;
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO quota_limits
            (scope_type, scope_id, storage_bytes, concurrent_runs, monthly_tokens, monthly_cost_micros, hard_cost_limit, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, now())
        ON CONFLICT (scope_type, scope_id) DO UPDATE SET
            storage_bytes = EXCLUDED.storage_bytes,
            concurrent_runs = EXCLUDED.concurrent_runs,
            monthly_tokens = EXCLUDED.monthly_tokens,
            monthly_cost_micros = EXCLUDED.monthly_cost_micros,
            hard_cost_limit = EXCLUDED.hard_cost_limit,
            updated_at = now()
        "#,
    )
    .bind(quota_scope_name(scope))
    .bind(scope_id)
    .bind(storage_bytes)
    .bind(concurrent_runs)
    .bind(monthly_tokens)
    .bind(monthly_cost)
    .bind(request.hard_cost_limit)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, 'quota.update', $3, $4, $5)")
        .bind(Uuid::new_v4()).bind(principal.user_id).bind(format!("{}_quota", quota_scope_name(scope))).bind(scope_id)
        .bind(json!({"storage_bytes": request.storage_bytes, "concurrent_runs": request.concurrent_runs, "monthly_tokens": request.monthly_tokens, "monthly_cost_micros": request.monthly_cost_micros, "hard_cost_limit": request.hard_cost_limit}))
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(quota_status(&state.pool, scope, scope_id).await?))
}

pub async fn enforce_run_quota_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    project_id: Uuid,
) -> Result<(), ApiError> {
    let team_id = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT team_id FROM projects WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    let mut scopes = vec![(QuotaScopeType::User, user_id)];
    if let Some(team_id) = team_id {
        scopes.push((QuotaScopeType::Team, team_id));
    }
    for (scope, scope_id) in scopes {
        lock_quota(tx, scope, scope_id).await?;
        let limit = sqlx::query_scalar::<_, Option<i32>>(
            "SELECT concurrent_runs FROM quota_limits WHERE scope_type = $1 AND scope_id = $2",
        )
        .bind(quota_scope_name(scope))
        .bind(scope_id)
        .fetch_optional(&mut **tx)
        .await?
        .flatten();
        let Some(limit) = limit else { continue };
        let running = match scope {
            QuotaScopeType::User => sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM runs WHERE creator_user_id = $1 AND state NOT IN ('completed','failed','canceled','expired')",
            )
            .bind(scope_id)
            .fetch_one(&mut **tx)
            .await?,
            QuotaScopeType::Team => sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM runs run JOIN projects project ON project.id = run.project_id WHERE project.team_id = $1 AND run.state NOT IN ('completed','failed','canceled','expired')",
            )
            .bind(scope_id)
            .fetch_one(&mut **tx)
            .await?,
        };
        if running >= i64::from(limit) {
            return Err(ApiError::Quota(format!(
                "{} concurrent run quota of {limit} is exhausted",
                quota_scope_name(scope),
            )));
        }
    }
    Ok(())
}

pub async fn enforce_storage_quota_tx(
    tx: &mut Transaction<'_, Postgres>,
    scope_type: &str,
    scope_id: Uuid,
    additional_bytes: u64,
) -> Result<(), ApiError> {
    let scope = parse_quota_scope(scope_type)?;
    lock_quota(tx, scope, scope_id).await?;
    let limit = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT storage_bytes FROM quota_limits WHERE scope_type = $1 AND scope_id = $2",
    )
    .bind(scope_type)
    .bind(scope_id)
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    let Some(limit) = limit else { return Ok(()) };
    let used = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(sum(total_bytes), 0)::bigint FROM snapshot_manifests WHERE key_scope_type = $1 AND key_scope_id = $2 AND status IN ('uploading','ready') AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(scope_type)
    .bind(scope_id)
    .fetch_one(&mut **tx)
    .await?;
    let requested = i64::try_from(additional_bytes)
        .map_err(|_| ApiError::Unprocessable("snapshot size is too large".to_owned()))?;
    if used.saturating_add(requested) > limit {
        return Err(ApiError::Quota(format!(
            "{} storage quota of {limit} bytes would be exceeded",
            quota_scope_name(scope),
        )));
    }
    Ok(())
}

pub async fn ensure_model_quota_for_run(pool: &PgPool, run_id: Uuid) -> Result<(), ApiError> {
    let scopes = run_quota_scopes(pool, run_id).await?;
    let period_start = current_period_start();
    for (scope, scope_id) in scopes {
        if let Some(row) = sqlx::query(
            "SELECT monthly_tokens, monthly_cost_micros, hard_cost_limit FROM quota_limits WHERE scope_type = $1 AND scope_id = $2",
        )
        .bind(quota_scope_name(scope))
        .bind(scope_id)
        .fetch_optional(pool)
        .await?
        {
            let usage = sqlx::query(
                "SELECT tokens, cost_micros FROM quota_usage WHERE scope_type = $1 AND scope_id = $2 AND period_start = $3",
            )
            .bind(quota_scope_name(scope))
            .bind(scope_id)
            .bind(period_start)
            .fetch_optional(pool)
            .await?;
            let tokens = usage.as_ref().map(|value| value.get::<i64, _>("tokens")).unwrap_or(0);
            let cost = usage.as_ref().map(|value| value.get::<i64, _>("cost_micros")).unwrap_or(0);
            if row.try_get::<Option<i64>, _>("monthly_tokens")?.is_some_and(|limit| tokens >= limit) {
                return Err(ApiError::Quota(format!("{} monthly token quota is exhausted", quota_scope_name(scope))));
            }
            if row.try_get::<bool, _>("hard_cost_limit")?
                && row.try_get::<Option<i64>, _>("monthly_cost_micros")?.is_some_and(|limit| cost >= limit)
            {
                return Err(ApiError::Quota(format!("{} monthly cost quota is exhausted", quota_scope_name(scope))));
            }
        }
    }
    Ok(())
}

pub async fn record_model_usage_for_run(
    pool: &PgPool,
    run_id: Uuid,
    tokens: u64,
    cost_micros: u64,
) -> Result<(), ApiError> {
    let scopes = run_quota_scopes(pool, run_id).await?;
    let period_start = current_period_start();
    let tokens = i64::try_from(tokens).map_err(|error| ApiError::Internal(error.into()))?;
    let cost_micros =
        i64::try_from(cost_micros).map_err(|error| ApiError::Internal(error.into()))?;
    let mut tx = pool.begin().await?;
    for (scope, scope_id) in scopes {
        lock_quota(&mut tx, scope, scope_id).await?;
        sqlx::query(
            r#"
            INSERT INTO quota_usage (scope_type, scope_id, period_start, tokens, cost_micros, updated_at)
            VALUES ($1, $2, $3, $4, $5, now())
            ON CONFLICT (scope_type, scope_id, period_start) DO UPDATE SET
                tokens = quota_usage.tokens + EXCLUDED.tokens,
                cost_micros = quota_usage.cost_micros + EXCLUDED.cost_micros,
                updated_at = now()
            "#,
        )
        .bind(quota_scope_name(scope))
        .bind(scope_id)
        .bind(period_start)
        .bind(tokens)
        .bind(cost_micros)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn active_project_support_grant(
    pool: &PgPool,
    support_user_id: Uuid,
    project_id: Uuid,
) -> Result<Option<Uuid>, ApiError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM support_grants WHERE support_user_id = $1 AND project_id = $2 AND thread_id IS NULL AND revoked_at IS NULL AND expires_at > now() ORDER BY expires_at LIMIT 1",
    )
    .bind(support_user_id)
    .bind(project_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn active_thread_support_grant(
    pool: &PgPool,
    support_user_id: Uuid,
    thread_id: Uuid,
) -> Result<Option<Uuid>, ApiError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM support_grants WHERE support_user_id = $1 AND thread_id = $2 AND project_id IS NULL AND revoked_at IS NULL AND expires_at > now() ORDER BY expires_at LIMIT 1",
    )
    .bind(support_user_id)
    .bind(thread_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn audit_support_access(
    pool: &PgPool,
    support_user_id: Uuid,
    grant_id: Uuid,
    target_type: &str,
    target_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, 'support_grant.access', $3, $4, $5)")
        .bind(Uuid::new_v4()).bind(support_user_id).bind(target_type).bind(target_id)
        .bind(json!({"support_grant_id": grant_id})).execute(pool).await?;
    Ok(())
}

async fn ensure_quota_view(
    pool: &PgPool,
    user_id: Uuid,
    scope: QuotaScopeType,
    scope_id: Uuid,
) -> Result<(), ApiError> {
    if db::user_is_platform_admin(pool, user_id).await? {
        return Ok(());
    }
    let allowed = match scope {
        QuotaScopeType::User => user_id == scope_id,
        QuotaScopeType::Team => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM team_members WHERE team_id = $1 AND user_id = $2)",
            )
            .bind(scope_id)
            .bind(user_id)
            .fetch_one(pool)
            .await?
        }
    };
    if allowed {
        Ok(())
    } else {
        Err(ApiError::Unauthorized(
            "the current user cannot view this quota".to_owned(),
        ))
    }
}

async fn quota_status(
    pool: &PgPool,
    scope: QuotaScopeType,
    scope_id: Uuid,
) -> Result<QuotaStatus, ApiError> {
    let scope_name = quota_scope_name(scope);
    let now = Utc::now();
    let limits_row =
        sqlx::query("SELECT * FROM quota_limits WHERE scope_type = $1 AND scope_id = $2")
            .bind(scope_name)
            .bind(scope_id)
            .fetch_optional(pool)
            .await?;
    let period_start = current_period_start();
    let usage_row = sqlx::query("SELECT tokens, cost_micros, updated_at FROM quota_usage WHERE scope_type = $1 AND scope_id = $2 AND period_start = $3")
        .bind(scope_name).bind(scope_id).bind(period_start).fetch_optional(pool).await?;
    let storage_bytes = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(sum(total_bytes), 0)::bigint FROM snapshot_manifests WHERE key_scope_type = $1 AND key_scope_id = $2 AND status IN ('uploading','ready') AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(scope_name).bind(scope_id).fetch_one(pool).await?;
    let running_runs = match scope {
        QuotaScopeType::User => sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM runs WHERE creator_user_id = $1 AND state NOT IN ('completed','failed','canceled','expired')",
        ).bind(scope_id).fetch_one(pool).await?,
        QuotaScopeType::Team => sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM runs run JOIN projects project ON project.id = run.project_id WHERE project.team_id = $1 AND run.state NOT IN ('completed','failed','canceled','expired')",
        ).bind(scope_id).fetch_one(pool).await?,
    };
    let limits = if let Some(row) = limits_row {
        QuotaLimitsRecord {
            schema_version: SCHEMA_VERSION,
            scope_type: scope,
            scope_id,
            storage_bytes: optional_i64_to_u64(row.try_get("storage_bytes")?)?,
            concurrent_runs: row
                .try_get::<Option<i32>, _>("concurrent_runs")?
                .map(u32::try_from)
                .transpose()
                .map_err(|error| ApiError::Internal(error.into()))?,
            monthly_tokens: optional_i64_to_u64(row.try_get("monthly_tokens")?)?,
            monthly_cost_micros: optional_i64_to_u64(row.try_get("monthly_cost_micros")?)?,
            hard_cost_limit: row.try_get("hard_cost_limit")?,
            updated_at: row.try_get("updated_at")?,
        }
    } else {
        QuotaLimitsRecord {
            schema_version: SCHEMA_VERSION,
            scope_type: scope,
            scope_id,
            storage_bytes: None,
            concurrent_runs: None,
            monthly_tokens: None,
            monthly_cost_micros: None,
            hard_cost_limit: true,
            updated_at: now,
        }
    };
    Ok(QuotaStatus {
        schema_version: SCHEMA_VERSION,
        limits,
        usage: QuotaUsageRecord {
            schema_version: SCHEMA_VERSION,
            scope_type: scope,
            scope_id,
            period_start: period_start.to_string(),
            storage_bytes: u64::try_from(storage_bytes)
                .map_err(|error| ApiError::Internal(error.into()))?,
            running_runs: u32::try_from(running_runs)
                .map_err(|error| ApiError::Internal(error.into()))?,
            tokens: usage_row
                .as_ref()
                .map(|row| row.get::<i64, _>("tokens"))
                .map(u64::try_from)
                .transpose()
                .map_err(|error| ApiError::Internal(error.into()))?
                .unwrap_or(0),
            cost_micros: usage_row
                .as_ref()
                .map(|row| row.get::<i64, _>("cost_micros"))
                .map(u64::try_from)
                .transpose()
                .map_err(|error| ApiError::Internal(error.into()))?
                .unwrap_or(0),
            updated_at: usage_row
                .as_ref()
                .map(|row| row.get("updated_at"))
                .unwrap_or(now),
        },
    })
}

async fn run_quota_scopes(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Vec<(QuotaScopeType, Uuid)>, ApiError> {
    let row = sqlx::query("SELECT run.creator_user_id, project.team_id FROM runs run JOIN projects project ON project.id = run.project_id WHERE run.id = $1")
        .bind(run_id).fetch_optional(pool).await?
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id} was not found")))?;
    let mut scopes = vec![(QuotaScopeType::User, row.try_get("creator_user_id")?)];
    if let Some(team_id) = row.try_get::<Option<Uuid>, _>("team_id")? {
        scopes.push((QuotaScopeType::Team, team_id));
    }
    Ok(scopes)
}

async fn lock_quota(
    tx: &mut Transaction<'_, Postgres>,
    scope: QuotaScopeType,
    scope_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("quota:{}:{scope_id}", quota_scope_name(scope)))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn current_period_start() -> chrono::NaiveDate {
    Utc::now()
        .date_naive()
        .with_day(1)
        .expect("every month has a first day")
}

fn parse_quota_scope(value: &str) -> Result<QuotaScopeType, ApiError> {
    match value {
        "user" => Ok(QuotaScopeType::User),
        "team" => Ok(QuotaScopeType::Team),
        _ => Err(ApiError::NotFound(format!(
            "quota scope {value} was not found"
        ))),
    }
}

fn quota_scope_name(scope: QuotaScopeType) -> &'static str {
    match scope {
        QuotaScopeType::User => "user",
        QuotaScopeType::Team => "team",
    }
}

fn optional_u64_to_i64(value: Option<u64>, field: &str) -> Result<Option<i64>, ApiError> {
    value
        .map(i64::try_from)
        .transpose()
        .map_err(|_| ApiError::Unprocessable(format!("{field} is too large")))
}

fn optional_i64_to_u64(value: Option<i64>) -> Result<Option<u64>, ApiError> {
    value
        .map(u64::try_from)
        .transpose()
        .map_err(|error| ApiError::Internal(error.into()))
}

fn row_to_support_grant(row: &PgRow) -> Result<SupportGrantRecord, ApiError> {
    Ok(SupportGrantRecord {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        granted_by: row.try_get("granted_by")?,
        support_user_id: row.try_get("support_user_id")?,
        project_id: row.try_get("project_id")?,
        thread_id: row.try_get("thread_id")?,
        reason: row.try_get("reason")?,
        created_at: row.try_get("created_at")?,
        expires_at: row.try_get("expires_at")?,
        revoked_at: row.try_get("revoked_at")?,
    })
}
