//! Explicit, reversible-name updates and conservative archival. Archiving never
//! deletes customer data, historical infrastructure records or central audit.
use super::*;
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(super) struct Settings {
    id: Uuid,
    name: String,
    revision: i64,
    archived_at: Option<DateTime<Utc>>,
    archived_by: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Rename {
    name: String,
    expected_revision: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Archive {
    confirmation_name: String,
    expected_revision: i64,
}

fn settings(row: &sqlx::postgres::PgRow) -> Settings {
    Settings {
        id: row.get("id"),
        name: row.get("name"),
        revision: row.get("lifecycle_revision"),
        archived_at: row.get("archived_at"),
        archived_by: row.get("archived_by"),
    }
}

/// Organisation authority mutations share this row lock with archival.
pub(crate) async fn lock_active(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
) -> Result<(), ApiError> {
    sqlx::query(
        "SELECT id FROM management_organisations WHERE id=$1 AND archived_at IS NULL FOR UPDATE",
    )
    .bind(org)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(denied)?;
    Ok(())
}

/// Runtime reads retain active authority through their transaction without
/// serialising independent reads from the same organisation.
pub(crate) async fn share_active(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
) -> Result<(), ApiError> {
    sqlx::query(
        "SELECT id FROM management_organisations WHERE id=$1 AND archived_at IS NULL FOR SHARE",
    )
    .bind(org)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(denied)?;
    Ok(())
}

pub(super) async fn inspect(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<Settings>, ApiError> {
    membership(&state, &principal, org, false).await?;
    let row = sqlx::query("SELECT id,name,lifecycle_revision,archived_at,archived_by FROM management_organisations WHERE id=$1 AND archived_at IS NULL")
        .bind(org).fetch_optional(&state.pool).await?.ok_or_else(denied)?;
    Ok(Json(settings(&row)))
}

pub(super) async fn rename(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(request): Json<Rename>,
) -> Result<Json<Settings>, ApiError> {
    let proposed = name(&request.name)?;
    if request.expected_revision < 1 {
        return Err(invalid("a current organisation revision is required"));
    }
    let mut tx = state.pool.begin().await?;
    sso::authorize(&mut tx, org, &principal).await?;
    let row = sqlx::query("UPDATE management_organisations SET name=$2,lifecycle_revision=lifecycle_revision+1 WHERE id=$1 AND archived_at IS NULL AND lifecycle_revision=$3 RETURNING id,name,lifecycle_revision,archived_at,archived_by")
        .bind(org).bind(proposed).bind(request.expected_revision).fetch_optional(&mut *tx).await?
        .ok_or_else(||ApiError::Conflict("reload organisation settings before saving".into()))?;
    let result = settings(&row);
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "organisation.rename",
        org,
        json!({"name":result.name,"revision":result.revision}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(result))
}

async fn archive_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    actor: Uuid,
    request: &Archive,
) -> Result<Settings, ApiError> {
    lock_active(tx, org).await?;
    let row = sqlx::query("SELECT id,name,lifecycle_revision,archived_at,archived_by FROM management_organisations WHERE id=$1")
        .bind(org).fetch_one(&mut **tx).await?;
    let current = settings(&row);
    if current.revision != request.expected_revision {
        return Err(ApiError::Conflict(
            "reload organisation settings before archiving".into(),
        ));
    }
    if request.confirmation_name != current.name {
        return Err(invalid(
            "type the exact organisation name to confirm archival",
        ));
    }
    let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_instances WHERE organisation_id=$1 AND state<>'deleted') OR EXISTS(SELECT 1 FROM management_operations WHERE organisation_id=$1 AND state IN ('queued','running','reconciling')) OR EXISTS(SELECT 1 FROM management_rollouts WHERE organisation_id=$1 AND state IN ('running','paused','canceling')) OR EXISTS(SELECT 1 FROM management_execution_reservations WHERE organisation_id=$1 AND state='held') OR EXISTS(SELECT 1 FROM management_wallet_reservations WHERE organisation_id=$1 AND state IN ('reserved','held')) OR EXISTS(SELECT 1 FROM management_executor_hosts WHERE trust_domain=$1 AND enabled) OR EXISTS(SELECT 1 FROM directory_connectors WHERE organisation_id=$1 AND active) OR EXISTS(SELECT 1 FROM management_environment_jobs WHERE organisation_id=$1 AND state IN ('queued','running','held')) OR EXISTS(SELECT 1 FROM management_vm_jobs WHERE organisation_id=$1 AND state IN ('queued','running','held')) OR EXISTS(SELECT 1 FROM management_environment_executions WHERE organisation_id=$1 AND state IN ('queued','running','held')) OR EXISTS(SELECT 1 FROM management_instance_imports WHERE organisation_id=$1 AND phase NOT IN ('source_retained','aborted'))")
        .bind(org).fetch_one(&mut **tx).await?;
    if blocked {
        return Err(ApiError::Conflict("delete remaining instances, finish pending operations and wallet reservations, and disable organisation executor hosts and directory connectors before archiving".into()));
    }
    // Close unused admission paths before marking the organisation archived.
    // Tokens, memberships and historical rows remain available to audit.
    sqlx::query("UPDATE management_invitations SET revoked_at=clock_timestamp() WHERE organisation_id=$1 AND accepted_at IS NULL AND revoked_at IS NULL")
        .bind(org).execute(&mut **tx).await?;
    sqlx::query("UPDATE management_sso SET enabled=FALSE,auto_provision=FALSE,revision=revision+1 WHERE organisation_id=$1 AND enabled")
        .bind(org).execute(&mut **tx).await?;
    let changed = sqlx::query("UPDATE management_organisations SET archived_at=clock_timestamp(),archived_by=$2,lifecycle_revision=lifecycle_revision+1 WHERE id=$1 RETURNING id,name,lifecycle_revision,archived_at,archived_by")
        .bind(org).bind(actor).fetch_one(&mut **tx).await?;
    Ok(settings(&changed))
}

pub(super) async fn archive(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(request): Json<Archive>,
) -> Result<Json<Settings>, ApiError> {
    if request.expected_revision < 1 {
        return Err(invalid("a current organisation revision is required"));
    }
    let mut tx = state.pool.begin().await?;
    // Match ordinary policy/instance creation lock order before taking the org.
    rollouts::lock(&mut tx).await?;
    execution_admission::lock_global(&mut tx).await?;
    sso::authorize(&mut tx, org, &principal).await?;
    let result = archive_in_tx(&mut tx, org, principal.user_id, &request).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "organisation.archive",
        org,
        json!({"name":result.name,"revision":result.revision,"archived_at":result.archived_at}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(result))
}

pub(super) async fn capabilities(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    if principal.bootstrap {
        return Err(denied());
    }
    // A current account may inspect feature availability before it joins its
    // first organisation; access checks still apply on each feature endpoint.
    let session = principal.session_id.ok_or_else(denied)?;
    let mut tx = state.pool.begin().await?;
    crate::organisation_mfa::session_in_tx(&state, &mut tx, principal.user_id, session, false)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"schema_version":1,"organisation_settings":true,"organisation_archive":true,"resource_catalog":true,"environments":true,"environment_access":true,"directory":true,"organisation_mfa":true,"central_team_management":false,"wallets":true,"wallet":true,"wallet_enforcement":true}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{postgres::PgPoolOptions, PgPool};

    struct Database {
        pool: PgPool,
        admin: PgPool,
        schema: String,
    }
    impl Database {
        async fn new() -> Self {
            let url =
                std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated PostgreSQL test URL");
            let admin = PgPoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await
                .unwrap();
            let schema = format!("organisation_lifecycle_{}", Uuid::new_v4().simple());
            sqlx::query(&format!("CREATE SCHEMA {schema}"))
                .execute(&admin)
                .await
                .unwrap();
            let scope = schema.clone();
            let pool = PgPoolOptions::new()
                .max_connections(4)
                .after_connect(move |connection, _| {
                    let scope = scope.clone();
                    Box::pin(async move {
                        sqlx::query(&format!("SET search_path TO {scope}"))
                            .execute(connection)
                            .await?;
                        Ok(())
                    })
                })
                .connect(&url)
                .await
                .unwrap();
            sqlx::raw_sql(r#"
                CREATE TABLE users(id UUID PRIMARY KEY);
                CREATE TABLE management_organisations(id UUID PRIMARY KEY,name TEXT NOT NULL,policy JSONB NOT NULL DEFAULT '{}');
                CREATE TABLE management_members(organisation_id UUID NOT NULL,user_id UUID NOT NULL,role TEXT NOT NULL);
                CREATE TABLE management_instances(id UUID PRIMARY KEY,organisation_id UUID NOT NULL,state TEXT NOT NULL);
                CREATE TABLE management_instance_members(organisation_id UUID NOT NULL);
                CREATE TABLE management_invitations(organisation_id UUID NOT NULL,accepted_at TIMESTAMPTZ,revoked_at TIMESTAMPTZ);
                CREATE TABLE management_sso(organisation_id UUID NOT NULL,enabled BOOLEAN NOT NULL,auto_provision BOOLEAN NOT NULL,revision BIGINT NOT NULL);
                CREATE TABLE management_templates(organisation_id UUID NOT NULL);
                CREATE TABLE management_executor_hosts(trust_domain UUID NOT NULL,enabled BOOLEAN NOT NULL);
                CREATE TABLE directory_connectors(organisation_id UUID NOT NULL,active BOOLEAN NOT NULL);
                CREATE TABLE organisation_mfa_policies(organisation_id UUID NOT NULL);
                CREATE TABLE management_operations(organisation_id UUID NOT NULL,state TEXT NOT NULL);
                CREATE TABLE management_rollouts(organisation_id UUID NOT NULL,state TEXT NOT NULL);
                CREATE TABLE management_execution_reservations(organisation_id UUID NOT NULL,state TEXT NOT NULL);
                CREATE TABLE management_wallet_reservations(organisation_id UUID NOT NULL,state TEXT NOT NULL);
                CREATE TABLE management_environment_jobs(organisation_id UUID NOT NULL,state TEXT NOT NULL);
                CREATE TABLE management_vm_jobs(organisation_id UUID NOT NULL,state TEXT NOT NULL);
                CREATE TABLE management_environment_executions(organisation_id UUID NOT NULL,state TEXT NOT NULL);
                CREATE TABLE management_instance_imports(organisation_id UUID NOT NULL,phase TEXT NOT NULL);
            "#).execute(&pool).await.unwrap();
            sqlx::raw_sql(include_str!(
                "../../migrations/0072_organisation_lifecycle.sql"
            ))
            .execute(&pool)
            .await
            .unwrap();
            Self {
                pool,
                admin,
                schema,
            }
        }
        async fn finish(self, result: Result<(), tokio::task::JoinError>) {
            self.pool.close().await;
            sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
                .execute(&self.admin)
                .await
                .unwrap();
            self.admin.close().await;
            result.unwrap();
        }
    }

    async fn seed(pool: &PgPool) -> (Uuid, Uuid) {
        let org = Uuid::new_v4();
        let actor = Uuid::new_v4();
        sqlx::query("INSERT INTO users(id) VALUES($1)")
            .bind(actor)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO management_organisations(id,name) VALUES($1,'Example')")
            .bind(org)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO management_members VALUES($1,$2,'admin')")
            .bind(org)
            .bind(actor)
            .execute(pool)
            .await
            .unwrap();
        (org, actor)
    }

    #[tokio::test]
    #[ignore = "requires an isolated PostgreSQL COWORK_TEST_DATABASE_URL with schema creation permission"]
    async fn postgres_archive_checks_revision_activity_and_preserves_history() {
        let db = Database::new().await;
        let pool = db.pool.clone();
        let result = tokio::spawn(async move {
            let (org, actor) = seed(&pool).await;
            let request = Archive {
                expected_revision: 1,
                confirmation_name: "Example".into(),
            };
            for invalid_request in [
                Archive {
                    expected_revision: 2,
                    confirmation_name: "Example".into(),
                },
                Archive {
                    expected_revision: 1,
                    confirmation_name: "Wrong name".into(),
                },
            ] {
                let mut tx = pool.begin().await.unwrap();
                assert!(archive_in_tx(&mut tx, org, actor, &invalid_request)
                    .await
                    .is_err());
                tx.rollback().await.unwrap();
            }
            let instance = Uuid::new_v4();
            sqlx::query("INSERT INTO management_instances VALUES($1,$2,'stopped')")
                .bind(instance)
                .bind(org)
                .execute(&pool)
                .await
                .unwrap();
            let mut tx = pool.begin().await.unwrap();
            assert!(matches!(
                archive_in_tx(&mut tx, org, actor, &request).await,
                Err(ApiError::Conflict(_))
            ));
            tx.rollback().await.unwrap();
            sqlx::query("UPDATE management_instances SET state='deleted'")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO management_operations VALUES($1,'queued')")
                .bind(org)
                .execute(&pool)
                .await
                .unwrap();
            let mut tx = pool.begin().await.unwrap();
            assert!(matches!(
                archive_in_tx(&mut tx, org, actor, &request).await,
                Err(ApiError::Conflict(_))
            ));
            tx.rollback().await.unwrap();
            sqlx::query("UPDATE management_operations SET state='succeeded'")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO management_wallet_reservations VALUES($1,'held')")
                .bind(org)
                .execute(&pool)
                .await
                .unwrap();
            let mut tx = pool.begin().await.unwrap();
            assert!(matches!(
                archive_in_tx(&mut tx, org, actor, &request).await,
                Err(ApiError::Conflict(_))
            ));
            tx.rollback().await.unwrap();
            sqlx::query("UPDATE management_wallet_reservations SET state='settled'")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO management_invitations(organisation_id) VALUES($1)")
                .bind(org)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO management_sso VALUES($1,true,true,1)")
                .bind(org)
                .execute(&pool)
                .await
                .unwrap();
            let mut tx = pool.begin().await.unwrap();
            let archived = archive_in_tx(&mut tx, org, actor, &request).await.unwrap();
            assert_eq!(archived.revision, 2);
            assert_eq!(archived.archived_by, Some(actor));
            assert!(archived.archived_at.is_some());
            tx.commit().await.unwrap();
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM management_instances WHERE state='deleted'"
                )
                .fetch_one(&pool)
                .await
                .unwrap(),
                1
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM management_members")
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                1
            );
            assert!(sqlx::query_scalar::<_, bool>(
                "SELECT revoked_at IS NOT NULL FROM management_invitations"
            )
            .fetch_one(&pool)
            .await
            .unwrap());
            assert!(sqlx::query_scalar::<_, bool>(
                "SELECT NOT enabled AND NOT auto_provision AND revision=2 FROM management_sso"
            )
            .fetch_one(&pool)
            .await
            .unwrap());
            let mut tx = pool.begin().await.unwrap();
            assert!(lock_active(&mut tx, org).await.is_err());
            tx.rollback().await.unwrap();
            let denied = sqlx::query("INSERT INTO management_members VALUES($1,$2,'member')")
                .bind(org)
                .bind(actor)
                .execute(&pool)
                .await
                .unwrap_err();
            assert_eq!(
                denied.as_database_error().unwrap().code().as_deref(),
                Some("23514")
            );
        })
        .await;
        db.finish(result).await;
    }

    #[tokio::test]
    #[ignore = "requires an isolated PostgreSQL COWORK_TEST_DATABASE_URL with schema creation permission"]
    async fn postgres_archive_serializes_with_concurrent_instance_admission() {
        let db = Database::new().await;
        let pool = db.pool.clone();
        let result = tokio::spawn(async move {
            let (org, actor) = seed(&pool).await;
            let request = Archive {
                expected_revision: 1,
                confirmation_name: "Example".into(),
            };
            let mut archive = pool.begin().await.unwrap();
            archive_in_tx(&mut archive, org, actor, &request)
                .await
                .unwrap();
            let other = pool.clone();
            let mut insert = tokio::spawn(async move {
                sqlx::query("INSERT INTO management_instances VALUES($1,$2,'pending')")
                    .bind(Uuid::new_v4())
                    .bind(org)
                    .execute(&other)
                    .await
            });
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), &mut insert)
                    .await
                    .is_err()
            );
            archive.commit().await.unwrap();
            let denied = insert.await.unwrap().unwrap_err();
            assert_eq!(
                denied.as_database_error().unwrap().code().as_deref(),
                Some("23514")
            );

            let (org, actor) = seed(&pool).await;
            let mut insert = pool.begin().await.unwrap();
            sqlx::query("INSERT INTO management_instances VALUES($1,$2,'pending')")
                .bind(Uuid::new_v4())
                .bind(org)
                .execute(&mut *insert)
                .await
                .unwrap();
            let other = pool.clone();
            let mut archive = tokio::spawn(async move {
                let mut tx = other.begin().await.unwrap();
                archive_in_tx(&mut tx, org, actor, &request).await
            });
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), &mut archive)
                    .await
                    .is_err()
            );
            insert.commit().await.unwrap();
            assert!(matches!(archive.await.unwrap(), Err(ApiError::Conflict(_))));
        })
        .await;
        db.finish(result).await;
    }
}
