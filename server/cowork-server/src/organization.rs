use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use cowork_contracts::{
    CreateExecutorPoolRequest, CreateProjectRequest, CreateTeamRequest, CreateThreadRequest,
    ExecutorKind, ExecutorPool, ExecutorTarget, GrantExecutorPoolRequest, ProjectPrivacy,
    ProjectRecord, ProjectRole, SetProjectMemberRequest, SetTeamMemberRequest, TeamRecord,
    TeamRole, ThreadRecord, UpdateProjectRequest, UpdateThreadRequest, SCHEMA_VERSION,
};
use serde::Deserialize;
use serde_json::Value;
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

use crate::{auth::Principal, db, error::ApiError, governance, sync, AppState};

#[derive(Debug, Deserialize)]
pub struct RevisionQuery {
    expected_revision: i64,
}

pub async fn create_team(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateTeamRequest>,
) -> Result<(StatusCode, Json<TeamRecord>), ApiError> {
    let name = validated_name(&request.name, "team")?;
    let id = Uuid::new_v4();
    let now = Utc::now();
    let etag = format!("W/\"{id}:1\"");
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        INSERT INTO teams (id, etag, name, owner_user_id, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $5) RETURNING *
        "#,
    )
    .bind(id)
    .bind(&etag)
    .bind(name)
    .bind(principal.user_id)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, 'owner')")
        .bind(id)
        .bind(principal.user_id)
        .execute(&mut *tx)
        .await?;
    let record = row_to_team(&row)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(record)))
}

pub async fn list_teams(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<TeamRecord>>, ApiError> {
    let rows = sqlx::query(
        r#"
            SELECT team.* FROM teams team
            JOIN team_members member ON member.team_id = team.id
            WHERE member.user_id = $1 AND team.deleted_at IS NULL
            ORDER BY team.name
            "#,
    )
    .bind(principal.user_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.iter().map(row_to_team).collect::<Result<_, _>>()?,
    ))
}

pub async fn set_team_member(
    State(state): State<AppState>,
    Path(team_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<SetTeamMemberRequest>,
) -> Result<StatusCode, ApiError> {
    ensure_team_admin(&state.pool, principal.user_id, team_id).await?;
    if request.role == TeamRole::Owner {
        return Err(ApiError::Unprocessable(
            "team ownership transfer requires a dedicated ownership operation".to_owned(),
        ));
    }
    sqlx::query(
        r#"
        INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, $3)
        ON CONFLICT (team_id, user_id) DO UPDATE SET role = EXCLUDED.role
        "#,
    )
    .bind(team_id)
    .bind(request.user_id)
    .bind(team_role_name(request.role))
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn create_project(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<ProjectRecord>), ApiError> {
    let name = validated_name(&request.name, "project")?;
    let description = validated_description(&request.description)?;
    let policy = validated_project_policy(request.policy)?;
    match request.privacy {
        ProjectPrivacy::PrivateLocal if request.team_id.is_some() => {
            return Err(ApiError::Unprocessable(
                "private projects cannot belong to a team".to_owned(),
            ));
        }
        ProjectPrivacy::TeamManaged => {
            let team_id = request.team_id.ok_or_else(|| {
                ApiError::Unprocessable("team projects require team_id".to_owned())
            })?;
            ensure_team_admin(&state.pool, principal.user_id, team_id).await?;
        }
        _ => {}
    }
    let id = Uuid::new_v4();
    let now = Utc::now();
    let etag = format!("W/\"{id}:1\"");
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        INSERT INTO projects (
            id, etag, owner_user_id, team_id, privacy, name, description,
            preferred_executor_target, policy, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(etag)
    .bind(principal.user_id)
    .bind(request.team_id)
    .bind(project_privacy_name(request.privacy))
    .bind(name)
    .bind(description)
    .bind(
        request
            .preferred_executor_target
            .map(serde_json::to_value)
            .transpose()?,
    )
    .bind(policy)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    sync::publish_canonical_project_tx(&mut tx, id).await?;
    let project = row_to_project(&row)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(project)))
}

pub async fn list_projects(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<ProjectRecord>>, ApiError> {
    let rows = sqlx::query(
            r#"
            SELECT DISTINCT project.* FROM projects project
            LEFT JOIN project_members pm ON pm.project_id = project.id AND pm.user_id = $1
            LEFT JOIN team_members tm ON tm.team_id = project.team_id AND tm.user_id = $1
            LEFT JOIN support_grants sg ON sg.support_user_id = $1
              AND sg.project_id = project.id AND sg.thread_id IS NULL
              AND sg.revoked_at IS NULL AND sg.expires_at > now()
            LEFT JOIN threads support_thread ON support_thread.project_id = project.id AND support_thread.deleted_at IS NULL
            LEFT JOIN support_grants tsg ON tsg.support_user_id = $1
              AND tsg.thread_id = support_thread.id AND tsg.project_id IS NULL
              AND tsg.revoked_at IS NULL AND tsg.expires_at > now()
            WHERE project.deleted_at IS NULL
              AND (project.owner_user_id = $1 OR pm.user_id IS NOT NULL OR tm.user_id IS NOT NULL
                   OR sg.id IS NOT NULL OR tsg.id IS NOT NULL)
            ORDER BY project.updated_at DESC
            "#,
        )
        .bind(principal.user_id)
        .fetch_all(&state.pool)
        .await?;
    for row in &rows {
        let project_id: Uuid = row.try_get("id")?;
        match ensure_project_role(
            &state.pool,
            principal.user_id,
            project_id,
            ProjectRole::Viewer,
        )
        .await
        {
            Ok(_) => {}
            Err(ApiError::Unauthorized(_)) => {
                let grant = sqlx::query(
                    r#"
                    SELECT grant_row.id, grant_row.thread_id FROM support_grants grant_row
                    JOIN threads thread ON thread.id = grant_row.thread_id
                    WHERE grant_row.support_user_id = $1 AND thread.project_id = $2
                      AND grant_row.project_id IS NULL AND grant_row.revoked_at IS NULL
                      AND grant_row.expires_at > now() AND thread.deleted_at IS NULL
                    ORDER BY grant_row.expires_at LIMIT 1
                    "#,
                )
                .bind(principal.user_id)
                .bind(project_id)
                .fetch_optional(&state.pool)
                .await?
                .ok_or_else(|| {
                    ApiError::Unauthorized("the current user cannot access this project".to_owned())
                })?;
                governance::audit_support_access(
                    &state.pool,
                    principal.user_id,
                    grant.try_get("id")?,
                    "thread",
                    grant.try_get("thread_id")?,
                )
                .await?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(Json(
        rows.iter().map(row_to_project).collect::<Result<_, _>>()?,
    ))
}

pub async fn get_project(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<ProjectRecord>, ApiError> {
    ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Viewer,
    )
    .await?;
    let row = sqlx::query("SELECT * FROM projects WHERE id = $1 AND deleted_at IS NULL")
        .bind(project_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project {project_id} was not found")))?;
    Ok(Json(row_to_project(&row)?))
}

pub async fn update_project(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<UpdateProjectRequest>,
) -> Result<Json<ProjectRecord>, ApiError> {
    ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    let name = validated_name(&request.name, "project")?;
    let description = validated_description(&request.description)?;
    let policy = validated_project_policy(request.policy)?;
    if request.expected_revision < 1 {
        return Err(ApiError::Unprocessable(
            "expected_revision must be positive".to_owned(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        UPDATE projects
        SET revision = revision + 1,
            etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
            name = $3, description = $4, preferred_executor_target = $5,
            policy = $6, updated_at = now()
        WHERE id = $1 AND revision = $2 AND deleted_at IS NULL
        RETURNING *
        "#,
    )
    .bind(project_id)
    .bind(request.expected_revision)
    .bind(name)
    .bind(description)
    .bind(
        request
            .preferred_executor_target
            .map(serde_json::to_value)
            .transpose()?,
    )
    .bind(policy)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ApiError::Conflict("project revision changed; reload before updating".to_owned())
    })?;
    sync::publish_canonical_project_tx(&mut tx, project_id).await?;
    let project = row_to_project(&row)?;
    tx.commit().await?;
    Ok(Json(project))
}

pub async fn delete_project(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    axum::extract::Query(query): axum::extract::Query<RevisionQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, ApiError> {
    ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    if query.expected_revision < 1 {
        return Err(ApiError::Unprocessable(
            "expected_revision must be positive".to_owned(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    let project = sqlx::query(
        "SELECT owner_user_id, privacy, revision FROM projects WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("project {project_id} was not found")))?;
    if project.try_get::<i64, _>("revision")? != query.expected_revision {
        return Err(ApiError::Conflict(
            "project revision changed; reload before deleting".to_owned(),
        ));
    }
    let owner_user_id: Uuid = project.try_get("owner_user_id")?;
    let private = project.try_get::<&str, _>("privacy")? == "private_local";
    let thread_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM threads WHERE project_id = $1 AND deleted_at IS NULL ORDER BY id FOR UPDATE",
    )
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    let message_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT message.id FROM messages message
        JOIN threads thread ON thread.id = message.thread_id
        WHERE thread.project_id = $1 AND message.deleted_at IS NULL
        ORDER BY message.id FOR UPDATE OF message
        "#,
    )
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE messages AS message
        SET revision = message.revision + 1,
            etag = 'W/"' || message.id::text || ':' || (message.revision + 1)::text || '"',
            deleted_at = now(), updated_at = now()
        FROM threads thread
        WHERE message.thread_id = thread.id AND thread.project_id = $1
          AND message.deleted_at IS NULL
        "#,
    )
    .bind(project_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE threads
        SET revision = revision + 1,
            etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
            deleted_at = now(), updated_at = now()
        WHERE project_id = $1 AND deleted_at IS NULL
        "#,
    )
    .bind(project_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE projects
        SET revision = revision + 1,
            etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
            deleted_at = now(), updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(project_id)
    .execute(&mut *tx)
    .await?;
    if private {
        for message_id in message_ids {
            sync::publish_server_tombstone_tx(&mut tx, owner_user_id, "message", message_id)
                .await?;
        }
        for thread_id in thread_ids {
            sync::publish_server_tombstone_tx(&mut tx, owner_user_id, "thread", thread_id).await?;
        }
        sync::publish_server_tombstone_tx(&mut tx, owner_user_id, "project", project_id).await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn set_project_member(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<SetProjectMemberRequest>,
) -> Result<StatusCode, ApiError> {
    ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO project_members (project_id, user_id, role) VALUES ($1, $2, $3)
        ON CONFLICT (project_id, user_id) DO UPDATE SET role = EXCLUDED.role
        "#,
    )
    .bind(project_id)
    .bind(request.user_id)
    .bind(project_role_name(request.role))
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn create_thread(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateThreadRequest>,
) -> Result<(StatusCode, Json<ThreadRecord>), ApiError> {
    ensure_project_role(
        &state.pool,
        principal.user_id,
        request.project_id,
        ProjectRole::Runner,
    )
    .await?;
    let title = validated_name(&request.title, "thread")?;
    if let Some(parent) = request.forked_from_thread_id {
        ensure_thread_project(&state.pool, parent, request.project_id).await?;
    }
    if let Some(message_id) = request.forked_from_message_id {
        let belongs = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM messages message
                JOIN threads thread ON thread.id = message.thread_id
                WHERE message.id = $1 AND thread.project_id = $2
            )
            "#,
        )
        .bind(message_id)
        .bind(request.project_id)
        .fetch_one(&state.pool)
        .await?;
        if !belongs {
            return Err(ApiError::Unprocessable(
                "forked message does not belong to the project".to_owned(),
            ));
        }
    }
    let id = Uuid::new_v4();
    let etag = format!("W/\"{id}:1\"");
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        INSERT INTO threads (
            id, etag, project_id, created_by, forked_from_thread_id,
            forked_from_message_id, title
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(etag)
    .bind(request.project_id)
    .bind(principal.user_id)
    .bind(request.forked_from_thread_id)
    .bind(request.forked_from_message_id)
    .bind(title)
    .fetch_one(&mut *tx)
    .await?;
    sync::publish_canonical_thread_tx(&mut tx, id).await?;
    let thread = row_to_thread(&row)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(thread)))
}

pub async fn update_thread(
    State(state): State<AppState>,
    Path(thread_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<UpdateThreadRequest>,
) -> Result<Json<ThreadRecord>, ApiError> {
    if request.expected_revision < 1 {
        return Err(ApiError::Unprocessable(
            "expected_revision must be positive".to_owned(),
        ));
    }
    let project_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT project_id FROM threads WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(thread_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("thread {thread_id} was not found")))?;
    ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    let title = validated_name(&request.title, "thread")?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        UPDATE threads
        SET revision = revision + 1,
            etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
            title = $3, updated_at = now()
        WHERE id = $1 AND revision = $2 AND deleted_at IS NULL
        RETURNING *
        "#,
    )
    .bind(thread_id)
    .bind(request.expected_revision)
    .bind(title)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ApiError::Conflict("thread revision changed; reload before updating".to_owned())
    })?;
    sync::publish_canonical_thread_tx(&mut tx, thread_id).await?;
    let thread = row_to_thread(&row)?;
    tx.commit().await?;
    Ok(Json(thread))
}

pub async fn delete_thread(
    State(state): State<AppState>,
    Path(thread_id): Path<Uuid>,
    axum::extract::Query(query): axum::extract::Query<RevisionQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, ApiError> {
    if query.expected_revision < 1 {
        return Err(ApiError::Unprocessable(
            "expected_revision must be positive".to_owned(),
        ));
    }
    let thread = sqlx::query(
        r#"
        SELECT thread.project_id, thread.revision, project.owner_user_id, project.privacy
        FROM threads thread
        JOIN projects project ON project.id = thread.project_id
        WHERE thread.id = $1 AND thread.deleted_at IS NULL AND project.deleted_at IS NULL
        "#,
    )
    .bind(thread_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("thread {thread_id} was not found")))?;
    let project_id: Uuid = thread.try_get("project_id")?;
    ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    if thread.try_get::<i64, _>("revision")? != query.expected_revision {
        return Err(ApiError::Conflict(
            "thread revision changed; reload before deleting".to_owned(),
        ));
    }
    let owner_user_id: Uuid = thread.try_get("owner_user_id")?;
    let private = thread.try_get::<&str, _>("privacy")? == "private_local";
    let mut tx = state.pool.begin().await?;
    let locked_revision = sqlx::query_scalar::<_, i64>(
        "SELECT revision FROM threads WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(thread_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("thread {thread_id} was not found")))?;
    if locked_revision != query.expected_revision {
        return Err(ApiError::Conflict(
            "thread revision changed; reload before deleting".to_owned(),
        ));
    }
    let message_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM messages WHERE thread_id = $1 AND deleted_at IS NULL ORDER BY id FOR UPDATE",
    )
    .bind(thread_id)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE messages
        SET revision = revision + 1,
            etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
            deleted_at = now(), updated_at = now()
        WHERE thread_id = $1 AND deleted_at IS NULL
        "#,
    )
    .bind(thread_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE threads
        SET revision = revision + 1,
            etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
            deleted_at = now(), updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(thread_id)
    .execute(&mut *tx)
    .await?;
    if private {
        for message_id in message_ids {
            sync::publish_server_tombstone_tx(&mut tx, owner_user_id, "message", message_id)
                .await?;
        }
        sync::publish_server_tombstone_tx(&mut tx, owner_user_id, "thread", thread_id).await?;
        sync::publish_canonical_project_tx(&mut tx, project_id).await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_project_threads(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<ThreadRecord>>, ApiError> {
    let rows = match ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Viewer,
    )
    .await
    {
        Ok(_) => sqlx::query(
            "SELECT * FROM threads WHERE project_id = $1 AND deleted_at IS NULL ORDER BY created_at DESC",
        )
        .bind(project_id)
        .fetch_all(&state.pool)
        .await?,
        Err(ApiError::Unauthorized(_)) => {
            let rows = sqlx::query(
                r#"
                SELECT thread.* FROM threads thread
                JOIN support_grants grant_row ON grant_row.thread_id = thread.id
                WHERE thread.project_id = $1 AND thread.deleted_at IS NULL
                  AND grant_row.support_user_id = $2 AND grant_row.project_id IS NULL
                  AND grant_row.revoked_at IS NULL AND grant_row.expires_at > now()
                ORDER BY thread.created_at DESC
                "#,
            )
            .bind(project_id)
            .bind(principal.user_id)
            .fetch_all(&state.pool)
            .await?;
            if rows.is_empty() {
                return Err(ApiError::Unauthorized(
                    "the current user cannot access this project".to_owned(),
                ));
            }
            for row in &rows {
                let thread_id: Uuid = row.try_get("id")?;
                let grant_id = governance::active_thread_support_grant(
                    &state.pool,
                    principal.user_id,
                    thread_id,
                )
                .await?
                .ok_or_else(|| ApiError::Unauthorized(
                    "the current user cannot access this thread".to_owned(),
                ))?;
                governance::audit_support_access(
                    &state.pool,
                    principal.user_id,
                    grant_id,
                    "thread",
                    thread_id,
                )
                .await?;
            }
            rows
        }
        Err(error) => return Err(error),
    };
    Ok(Json(
        rows.iter().map(row_to_thread).collect::<Result<_, _>>()?,
    ))
}

pub async fn create_executor_pool(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateExecutorPoolRequest>,
) -> Result<(StatusCode, Json<ExecutorPool>), ApiError> {
    if request.kind == ExecutorKind::PersonalDevice {
        return Err(ApiError::Unprocessable(
            "personal devices do not belong to managed executor pools".to_owned(),
        ));
    }
    if let Some(team_id) = request.team_id {
        ensure_team_admin(&state.pool, principal.user_id, team_id).await?;
    } else if !db::user_is_platform_admin(&state.pool, principal.user_id).await? {
        return Err(ApiError::Unauthorized(
            "only platform administrators can create global executor pools".to_owned(),
        ));
    }
    let name = validated_name(&request.name, "executor pool")?;
    let id = Uuid::new_v4();
    let etag = format!("W/\"{id}:1\"");
    let row = sqlx::query(
        r#"
        INSERT INTO executor_pools (
            id, etag, name, kind, team_id, policy, created_by_user_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *
        "#,
    )
    .bind(id)
    .bind(etag)
    .bind(name)
    .bind(executor_kind_name(request.kind))
    .bind(request.team_id)
    .bind(request.policy)
    .bind(principal.user_id)
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(row_to_pool(&row)?)))
}

pub async fn list_executor_pools(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<ExecutorPool>>, ApiError> {
    let rows = if db::user_is_platform_admin(&state.pool, principal.user_id).await? {
        sqlx::query("SELECT * FROM executor_pools WHERE deleted_at IS NULL ORDER BY name")
            .fetch_all(&state.pool)
            .await?
    } else {
        sqlx::query(
            r#"
            SELECT DISTINCT pool.* FROM executor_pools pool
            LEFT JOIN team_members tm ON tm.team_id = pool.team_id AND tm.user_id = $1
            LEFT JOIN executor_pool_project_grants grant_row ON grant_row.pool_id = pool.id
            LEFT JOIN projects project ON project.id = grant_row.project_id
            LEFT JOIN project_members pm ON pm.project_id = project.id AND pm.user_id = $1
            LEFT JOIN team_members project_tm ON project_tm.team_id = project.team_id AND project_tm.user_id = $1
            WHERE pool.deleted_at IS NULL
              AND (tm.user_id IS NOT NULL OR project.owner_user_id = $1 OR pm.user_id IS NOT NULL OR project_tm.user_id IS NOT NULL)
            ORDER BY pool.name
            "#,
        )
        .bind(principal.user_id)
        .fetch_all(&state.pool)
        .await?
    };
    Ok(Json(
        rows.iter().map(row_to_pool).collect::<Result<_, _>>()?,
    ))
}

pub async fn grant_executor_pool(
    State(state): State<AppState>,
    Path(pool_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<GrantExecutorPoolRequest>,
) -> Result<StatusCode, ApiError> {
    ensure_pool_admin(&state.pool, principal.user_id, pool_id).await?;
    ensure_project_role(
        &state.pool,
        principal.user_id,
        request.project_id,
        ProjectRole::Editor,
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO executor_pool_project_grants (pool_id, project_id, granted_by_user_id)
        VALUES ($1, $2, $3) ON CONFLICT (pool_id, project_id) DO NOTHING
        "#,
    )
    .bind(pool_id)
    .bind(request.project_id)
    .bind(principal.user_id)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn ensure_project_role(
    pool: &PgPool,
    user_id: Uuid,
    project_id: Uuid,
    required: ProjectRole,
) -> Result<ProjectRole, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT project.owner_user_id, pm.role AS project_role, tm.role AS team_role
        FROM projects project
        LEFT JOIN project_members pm ON pm.project_id = project.id AND pm.user_id = $2
        LEFT JOIN team_members tm ON tm.team_id = project.team_id AND tm.user_id = $2
        WHERE project.id = $1 AND project.deleted_at IS NULL
        "#,
    )
    .bind(project_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("project {project_id} was not found")))?;
    let role = if row.get::<Uuid, _>("owner_user_id") == user_id {
        Some(ProjectRole::Editor)
    } else if let Some(role) = row.try_get::<Option<String>, _>("project_role")? {
        Some(parse_project_role(&role)?)
    } else {
        match row.try_get::<Option<String>, _>("team_role")?.as_deref() {
            Some("owner" | "admin") => Some(ProjectRole::Editor),
            Some("member") => Some(ProjectRole::Viewer),
            _ => None,
        }
    };
    let role = if let Some(role) = role {
        role
    } else if required == ProjectRole::Viewer {
        if let Some(grant_id) =
            governance::active_project_support_grant(pool, user_id, project_id).await?
        {
            governance::audit_support_access(pool, user_id, grant_id, "project", project_id)
                .await?;
            ProjectRole::Viewer
        } else {
            return Err(ApiError::Unauthorized(
                "the current user cannot access this project".to_owned(),
            ));
        }
    } else {
        return Err(ApiError::Unauthorized(
            "the current user cannot access this project".to_owned(),
        ));
    };
    if role_rank(role) < role_rank(required) {
        return Err(ApiError::Unauthorized(format!(
            "project role {} is required",
            project_role_name(required)
        )));
    }
    Ok(role)
}

pub async fn ensure_thread_role(
    pool: &PgPool,
    user_id: Uuid,
    project_id: Uuid,
    thread_id: Uuid,
    required: ProjectRole,
) -> Result<ProjectRole, ApiError> {
    ensure_thread_project(pool, thread_id, project_id).await?;
    match ensure_project_role(pool, user_id, project_id, required).await {
        Ok(role) => Ok(role),
        Err(ApiError::Unauthorized(_)) if required == ProjectRole::Viewer => {
            let grant_id = governance::active_thread_support_grant(pool, user_id, thread_id)
                .await?
                .ok_or_else(|| {
                    ApiError::Unauthorized("the current user cannot access this thread".to_owned())
                })?;
            governance::audit_support_access(pool, user_id, grant_id, "thread", thread_id).await?;
            Ok(ProjectRole::Viewer)
        }
        Err(error) => Err(error),
    }
}

pub async fn ensure_thread_project(
    pool: &PgPool,
    thread_id: Uuid,
    project_id: Uuid,
) -> Result<(), ApiError> {
    let valid = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM threads WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL)",
    )
    .bind(thread_id)
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    if valid {
        Ok(())
    } else {
        Err(ApiError::Unprocessable(
            "thread does not belong to the selected project".to_owned(),
        ))
    }
}

pub async fn ensure_run_context(
    pool: &PgPool,
    user_id: Uuid,
    project_id: Uuid,
    thread_id: Uuid,
    project_revision: i64,
    privacy: ProjectPrivacy,
    target: &ExecutorTarget,
) -> Result<(), ApiError> {
    ensure_project_role(pool, user_id, project_id, ProjectRole::Runner).await?;
    ensure_thread_project(pool, thread_id, project_id).await?;
    let row =
        sqlx::query("SELECT revision, privacy FROM projects WHERE id = $1 AND deleted_at IS NULL")
            .bind(project_id)
            .fetch_one(pool)
            .await?;
    let current_revision: i64 = row.try_get("revision")?;
    let current_privacy = parse_project_privacy(row.try_get("privacy")?)?;
    if current_revision != project_revision {
        return Err(ApiError::Conflict(format!(
            "project revision changed from {project_revision} to {current_revision}"
        )));
    }
    if current_privacy != privacy {
        return Err(ApiError::Conflict(
            "project privacy does not match the persisted project".to_owned(),
        ));
    }
    match target {
        ExecutorTarget::ServerLinux {
            pool_id: Some(pool_id),
        } => {
            ensure_pool_allowed_for_project(pool, *pool_id, project_id, ExecutorKind::ServerLinux)
                .await?;
        }
        ExecutorTarget::ManagedWindowsPool { pool_id } => {
            ensure_pool_allowed_for_project(
                pool,
                *pool_id,
                project_id,
                ExecutorKind::ManagedWindows,
            )
            .await?;
        }
        ExecutorTarget::PersonalDevice { device_id } => {
            if !db::user_can_target_personal_device(pool, user_id, *device_id).await? {
                return Err(ApiError::Unauthorized(
                    "personal devices can only be targeted by their owner".to_owned(),
                ));
            }
        }
        ExecutorTarget::ServerLinux { pool_id: None } => {}
    }
    Ok(())
}

pub async fn ensure_pool_allowed_for_project(
    pool: &PgPool,
    pool_id: Uuid,
    project_id: Uuid,
    expected_kind: ExecutorKind,
) -> Result<(), ApiError> {
    let allowed = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM executor_pools pool
            JOIN executor_pool_project_grants grant_row ON grant_row.pool_id = pool.id
            WHERE pool.id = $1 AND grant_row.project_id = $2
              AND pool.kind = $3 AND pool.deleted_at IS NULL
        )
        "#,
    )
    .bind(pool_id)
    .bind(project_id)
    .bind(executor_kind_name(expected_kind))
    .fetch_one(pool)
    .await?;
    if allowed {
        Ok(())
    } else {
        Err(ApiError::Unauthorized(
            "executor pool is not granted to this project".to_owned(),
        ))
    }
}

pub(crate) async fn ensure_team_admin(
    pool: &PgPool,
    user_id: Uuid,
    team_id: Uuid,
) -> Result<(), ApiError> {
    let role = sqlx::query_scalar::<_, String>(
        "SELECT role FROM team_members WHERE team_id = $1 AND user_id = $2",
    )
    .bind(team_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    if matches!(role.as_deref(), Some("owner" | "admin")) {
        Ok(())
    } else {
        Err(ApiError::Unauthorized(
            "team owner or administrator role is required".to_owned(),
        ))
    }
}

async fn ensure_pool_admin(pool: &PgPool, user_id: Uuid, pool_id: Uuid) -> Result<(), ApiError> {
    if db::user_is_platform_admin(pool, user_id).await? {
        return Ok(());
    }
    let team_id = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT team_id FROM executor_pools WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(pool_id)
    .fetch_optional(pool)
    .await?
    .flatten()
    .ok_or_else(|| {
        ApiError::Unauthorized("global pool administration requires platform admin".to_owned())
    })?;
    ensure_team_admin(pool, user_id, team_id).await
}

fn row_to_team(row: &PgRow) -> Result<TeamRecord, ApiError> {
    Ok(TeamRecord {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        name: row.try_get("name")?,
        owner_user_id: row.try_get("owner_user_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

fn row_to_project(row: &PgRow) -> Result<ProjectRecord, ApiError> {
    Ok(ProjectRecord {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        owner_user_id: row.try_get("owner_user_id")?,
        team_id: row.try_get("team_id")?,
        privacy: parse_project_privacy(row.try_get("privacy")?)?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        preferred_executor_target: row
            .try_get::<Option<Value>, _>("preferred_executor_target")?
            .map(serde_json::from_value)
            .transpose()?,
        current_version_id: row.try_get("current_version_id")?,
        policy: row.try_get("policy")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

fn row_to_thread(row: &PgRow) -> Result<ThreadRecord, ApiError> {
    Ok(ThreadRecord {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        project_id: row.try_get("project_id")?,
        forked_from_thread_id: row.try_get("forked_from_thread_id")?,
        forked_from_message_id: row.try_get("forked_from_message_id")?,
        title: row.try_get("title")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

fn row_to_pool(row: &PgRow) -> Result<ExecutorPool, ApiError> {
    Ok(ExecutorPool {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        name: row.try_get("name")?,
        kind: parse_executor_kind(row.try_get("kind")?)?,
        team_id: row.try_get("team_id")?,
        policy: row.try_get("policy")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

fn validated_name<'a>(value: &'a str, entity: &str) -> Result<&'a str, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 200 {
        Err(ApiError::Unprocessable(format!(
            "{entity} name must contain 1 to 200 characters"
        )))
    } else {
        Ok(value)
    }
}

fn validated_description(value: &str) -> Result<&str, ApiError> {
    if value.len() > 200_000 {
        Err(ApiError::Unprocessable(
            "project description must not exceed 200000 characters".to_owned(),
        ))
    } else {
        Ok(value)
    }
}

fn validated_project_policy(policy: Value) -> Result<Value, ApiError> {
    if policy.is_null() {
        Ok(serde_json::json!({}))
    } else if policy.is_object() {
        Ok(policy)
    } else {
        Err(ApiError::Unprocessable(
            "project policy must be a JSON object".to_owned(),
        ))
    }
}

fn role_rank(role: ProjectRole) -> u8 {
    match role {
        ProjectRole::Viewer => 0,
        ProjectRole::Runner => 1,
        ProjectRole::Editor => 2,
    }
}

fn project_role_name(role: ProjectRole) -> &'static str {
    match role {
        ProjectRole::Viewer => "viewer",
        ProjectRole::Runner => "runner",
        ProjectRole::Editor => "editor",
    }
}

fn parse_project_role(value: &str) -> Result<ProjectRole, ApiError> {
    match value {
        "viewer" => Ok(ProjectRole::Viewer),
        "runner" => Ok(ProjectRole::Runner),
        "editor" => Ok(ProjectRole::Editor),
        other => Err(ApiError::Internal(anyhow::anyhow!(
            "unknown project role {other}"
        ))),
    }
}

fn team_role_name(role: TeamRole) -> &'static str {
    match role {
        TeamRole::Owner => "owner",
        TeamRole::Admin => "admin",
        TeamRole::Member => "member",
    }
}

fn project_privacy_name(privacy: ProjectPrivacy) -> &'static str {
    match privacy {
        ProjectPrivacy::PrivateLocal => "private_local",
        ProjectPrivacy::TeamManaged => "team_managed",
    }
}

fn parse_project_privacy(value: &str) -> Result<ProjectPrivacy, ApiError> {
    match value {
        "private_local" => Ok(ProjectPrivacy::PrivateLocal),
        "team_managed" => Ok(ProjectPrivacy::TeamManaged),
        other => Err(ApiError::Internal(anyhow::anyhow!(
            "unknown project privacy {other}"
        ))),
    }
}

fn executor_kind_name(kind: ExecutorKind) -> &'static str {
    match kind {
        ExecutorKind::ServerLinux => "server_linux",
        ExecutorKind::ManagedWindows => "managed_windows",
        ExecutorKind::PersonalDevice => "personal_device",
    }
}

fn parse_executor_kind(value: &str) -> Result<ExecutorKind, ApiError> {
    match value {
        "server_linux" => Ok(ExecutorKind::ServerLinux),
        "managed_windows" => Ok(ExecutorKind::ManagedWindows),
        "personal_device" => Ok(ExecutorKind::PersonalDevice),
        other => Err(ApiError::Internal(anyhow::anyhow!(
            "unknown executor kind {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_mutation_boundaries_are_fail_closed() {
        assert!(validated_name(" Project ", "project").is_ok());
        assert!(validated_name(" ", "project").is_err());
        assert!(validated_name(&"x".repeat(201), "project").is_err());
        assert!(validated_description(&"x".repeat(200_000)).is_ok());
        assert!(validated_description(&"x".repeat(200_001)).is_err());
        assert_eq!(
            validated_project_policy(Value::Null).expect("null policy defaults safely"),
            serde_json::json!({})
        );
        assert!(validated_project_policy(serde_json::json!({"tool_policy": "autonomous"})).is_ok());
        assert!(validated_project_policy(serde_json::json!(["not", "an", "object"])).is_err());
    }
}
