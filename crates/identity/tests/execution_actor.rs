//! Actual identity-store checks, including authority changes after a retained review.
use cowork_contracts::TeamExecutionActor;
use cowork_identity::*;
use sqlx::{AnyPool, any::AnyPoolOptions};
use uuid::Uuid;

struct Fixture {
    _directory: tempfile::TempDir,
    pool: AnyPool,
    store: IdentityStore,
    principal: Principal,
    organization: Uuid,
    project: Uuid,
    binding: TeamExecutionActor,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let (url, backend) = match std::env::var("COWORK_TEAM_ACTOR_DATABASE_URL") {
            Ok(url) => {
                assert!(
                    url.rsplit('/')
                        .next()
                        .is_some_and(|name| name.starts_with("cowork_team_actor_")),
                    "Use a new dedicated identity actor qualification database"
                );
                (url, BackendKind::Postgres)
            }
            Err(_) => {
                let path = directory.path().join("actors.sqlite3");
                let encoded = percent_encoding::utf8_percent_encode(
                    path.to_str().unwrap(),
                    percent_encoding::NON_ALPHANUMERIC,
                )
                .to_string();
                (format!("sqlite:{encoded}?mode=rwc"), BackendKind::Sqlite)
            }
        };
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(8)
            .after_connect(move |connection, _| {
                Box::pin(async move {
                    if backend == BackendKind::Sqlite {
                        for pragma in [
                            "PRAGMA foreign_keys=ON",
                            "PRAGMA busy_timeout=10000",
                            "PRAGMA journal_mode=WAL",
                        ] {
                            sqlx::query(pragma).execute(&mut *connection).await?;
                        }
                    }
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        migrations::apply(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        let store = IdentityStore::from_pool(pool.clone(), backend);
        let bootstrap = "synthetic-actor-bootstrap-credential-32-bytes";
        store
            .initialize_credentials(bootstrap, "synthetic-actor-settlement-credential-32-bytes")
            .await
            .unwrap();
        let (organization, issued) = store
            .bootstrap(
                bootstrap,
                "actor-root",
                "Actor root",
                "Synthetic-actor-password-for-disposable-test!",
                "Actor team",
            )
            .await
            .unwrap();
        let principal = store.authenticate_session(&issued.token).await.unwrap();
        let project = store
            .create_resource(
                &principal,
                organization.id,
                ContentKind::Project,
                "Actor project",
            )
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let binding = store
            .resolve_team_execution_actor_in_transaction(
                &mut tx,
                &principal,
                organization.id,
                project.id,
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        Self {
            _directory: directory,
            pool,
            store,
            principal,
            organization: organization.id,
            project: project.id,
            binding,
        }
    }
    async fn close(self) {
        self.pool.close().await;
    }
}

#[tokio::test]
async fn authenticated_resolution_preserves_exact_retained_actor_facts() {
    let f = Fixture::new().await;
    assert_eq!(f.binding.identity_id, f.principal.identity_id());
    assert_eq!(f.binding.session_id, f.principal.session_id());
    assert_eq!(f.binding.organization_id, f.organization);
    assert_eq!(f.binding.project_id, f.project);
    let mut tx = f.pool.begin().await.unwrap();
    let repeated = f
        .store
        .resolve_team_execution_actor_in_transaction(
            &mut tx,
            &f.principal,
            f.organization,
            f.project,
        )
        .await
        .unwrap();
    assert_eq!(repeated, f.binding);
    f.store
        .validate_retained_team_execution_actor_in_transaction(&mut tx, &f.binding)
        .await
        .unwrap();
    f.store
        .validate_retained_team_approver_in_transaction(
            &mut tx,
            f.binding.identity_id,
            f.binding.session_id,
            f.binding.session_revision,
            f.organization,
            f.project,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    f.close().await;
}

#[tokio::test]
async fn forged_or_stale_retained_actor_fields_are_rejected() {
    let f = Fixture::new().await;
    let value = serde_json::to_value(&f.binding).unwrap();
    for field in [
        "identity_id",
        "session_id",
        "organization_id",
        "membership_id",
        "project_id",
        "session_revision",
        "organization_epoch",
        "membership_revision",
        "resource_revision",
        "grant_revision",
    ] {
        let mut forged = value.clone();
        forged[field] = if field.ends_with("_id") {
            serde_json::json!(Uuid::new_v4())
        } else {
            serde_json::json!(value[field].as_i64().unwrap() + 1)
        };
        let forged: TeamExecutionActor = serde_json::from_value(forged).unwrap();
        let mut tx = f.pool.begin().await.unwrap();
        assert!(
            f.store
                .validate_retained_team_execution_actor_in_transaction(&mut tx, &forged)
                .await
                .is_err(),
            "{field}"
        );
        tx.rollback().await.unwrap();
    }
    f.close().await;
}

#[tokio::test]
async fn retained_actor_rechecks_current_expiry_revocation_roles_grants_and_clock() {
    let f = Fixture::new().await;
    let mutations = [
        (
            "session expiry",
            format!(
                "UPDATE identity_sessions SET expires_at=1 WHERE id='{}'",
                f.binding.session_id
            ),
        ),
        (
            "session revocation",
            format!(
                "UPDATE identity_sessions SET revoked=1 WHERE id='{}'",
                f.binding.session_id
            ),
        ),
        (
            "subject disabled",
            format!(
                "UPDATE identity_subjects SET active=0 WHERE id='{}'",
                f.binding.identity_id
            ),
        ),
        (
            "organization epoch",
            format!(
                "UPDATE identity_organizations SET authorization_epoch=authorization_epoch+1 WHERE id='{}'",
                f.organization
            ),
        ),
        (
            "membership revision",
            format!(
                "UPDATE identity_memberships SET revision=revision+1 WHERE id='{}'",
                f.binding.membership_id
            ),
        ),
        (
            "viewer ceiling",
            format!(
                "UPDATE identity_memberships SET role='viewer' WHERE id='{}'",
                f.binding.membership_id
            ),
        ),
        (
            "grant revoked",
            format!(
                "UPDATE identity_content_grants SET active=0 WHERE resource_id='{}'",
                f.project
            ),
        ),
        (
            "read removed",
            format!(
                "UPDATE identity_content_grants SET permissions=4 WHERE resource_id='{}'",
                f.project
            ),
        ),
        (
            "execute removed",
            format!(
                "UPDATE identity_content_grants SET permissions=1 WHERE resource_id='{}'",
                f.project
            ),
        ),
        (
            "resource revision",
            format!(
                "UPDATE identity_resources SET revision=revision+1 WHERE id='{}'",
                f.project
            ),
        ),
        (
            "clock rollback",
            "UPDATE identity_installation SET utc_highwater=utc_highwater+3600000 WHERE id=1"
                .into(),
        ),
    ];
    for (label, mutation) in mutations {
        let mut tx = f.pool.begin().await.unwrap();
        // Lock identity first, then simulate an authoritative change in this
        // disposable transaction, and require fresh validation before an effect.
        f.store
            .validate_retained_team_execution_actor_in_transaction(&mut tx, &f.binding)
            .await
            .unwrap();
        sqlx::query(&mutation).execute(&mut *tx).await.unwrap();
        let result = f
            .store
            .validate_retained_team_execution_actor_in_transaction(&mut tx, &f.binding)
            .await;
        assert!(result.is_err(), "{label}");
        if label == "clock rollback" {
            assert!(matches!(result, Err(IdentityError::ClockRollback)));
        }
        // Approvers intentionally need current rights, not an old membership/
        // resource epoch. Lost rights, session expiry and rollback still block.
        if !matches!(
            label,
            "organization epoch" | "membership revision" | "resource revision"
        ) {
            assert!(
                f.store
                    .validate_retained_team_approver_in_transaction(
                        &mut tx,
                        f.binding.identity_id,
                        f.binding.session_id,
                        f.binding.session_revision,
                        f.organization,
                        f.project
                    )
                    .await
                    .is_err(),
                "approver: {label}"
            );
        }
        tx.rollback().await.unwrap();
    }
    f.close().await;
}

#[tokio::test]
async fn scoped_sessions_cannot_expand_organization_or_read_only_ceiling() {
    let f = Fixture::new().await;
    let mut tx = f.pool.begin().await.unwrap();
    let issued = f
        .store
        .issue_verified_scoped_session_with_permissions_in_transaction(
            &mut tx,
            f.principal.identity_id(),
            f.organization,
            3_600_000,
            15,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let scoped = f.store.authenticate_session(&issued.token).await.unwrap();
    let mut tx = f.pool.begin().await.unwrap();
    let saved = f
        .store
        .resolve_team_execution_actor_in_transaction(&mut tx, &scoped, f.organization, f.project)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for permissions in [1, 15] {
        let mut tx = f.pool.begin().await.unwrap();
        if permissions == 1 {
            sqlx::query("UPDATE identity_session_scopes SET permissions=1 WHERE session_id=$1")
                .bind(scoped.session_id().to_string())
                .execute(&mut *tx)
                .await
                .unwrap();
            assert!(matches!(
                f.store
                    .resolve_team_execution_actor_in_transaction(
                        &mut tx,
                        &scoped,
                        f.organization,
                        f.project
                    )
                    .await,
                Err(IdentityError::Forbidden)
            ));
            assert!(matches!(
                f.store
                    .validate_retained_team_execution_actor_in_transaction(&mut tx, &saved)
                    .await,
                Err(IdentityError::Forbidden)
            ));
            assert!(matches!(
                f.store
                    .validate_retained_team_approver_in_transaction(
                        &mut tx,
                        saved.identity_id,
                        saved.session_id,
                        saved.session_revision,
                        saved.organization_id,
                        saved.project_id
                    )
                    .await,
                Err(IdentityError::Forbidden)
            ));
        } else {
            // A different organization cannot be substituted in a retained actor
            // even when the session still has all action bits in its real scope.
            let mut foreign = saved.clone();
            foreign.organization_id = Uuid::new_v4();
            assert!(matches!(
                f.store
                    .validate_retained_team_execution_actor_in_transaction(&mut tx, &foreign)
                    .await,
                Err(IdentityError::Forbidden)
            ));
        }
        tx.rollback().await.unwrap();
    }
    f.close().await;
}
