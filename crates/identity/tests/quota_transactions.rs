use cowork_identity::*;
use sqlx::{AnyPool, Row, any::AnyPoolOptions};
use std::{sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio::sync::Barrier;
use uuid::Uuid;

const BOOTSTRAP: &str = "synthetic-quota-tx-bootstrap-32-bytes-minimum";
const SETTLEMENT: &str = "synthetic-quota-tx-settlement-32-bytes-minimum";
const PASSWORD: &str = "Synthetic-quota-tx-password-not-for-use!";

struct Fixture {
    _directory: Option<TempDir>,
    pool: AnyPool,
    store: IdentityStore,
    actor: Principal,
    org: Organization,
    resource: ContentResource,
    permit: ContentPermit,
    expires_at: i64,
}

fn amounts(tokens: i64, runs: i64) -> QuotaAmounts {
    QuotaAmounts {
        model_tokens: tokens,
        cost_microunits: tokens * 2,
        storage_bytes: tokens * 3,
        concurrent_runs: runs,
    }
}

impl Fixture {
    async fn sqlite() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("identity-quota-tx.sqlite3");
        let encoded = percent_encoding::utf8_percent_encode(
            path.to_str().unwrap(),
            percent_encoding::NON_ALPHANUMERIC,
        );
        Self::new(
            &format!("sqlite:{encoded}?mode=rwc"),
            BackendKind::Sqlite,
            Some(directory),
        )
        .await
    }

    async fn new(url: &str, backend: BackendKind, directory: Option<TempDir>) -> Self {
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(8)
            .after_connect(move |connection, _| {
                Box::pin(async move {
                    if backend == BackendKind::Sqlite {
                        for pragma in [
                            "PRAGMA foreign_keys = ON",
                            "PRAGMA busy_timeout = 10000",
                            "PRAGMA journal_mode = WAL",
                            "PRAGMA synchronous = FULL",
                        ] {
                            sqlx::query(pragma).execute(&mut *connection).await?;
                        }
                    }
                    Ok(())
                })
            })
            .connect(url)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        migrations::apply(&mut tx).await.unwrap();
        sqlx::query(
            "CREATE TABLE quota_product_effects(id TEXT PRIMARY KEY, reservation_id TEXT NOT NULL)",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let store = IdentityStore::from_pool(pool.clone(), backend);
        store
            .initialize_credentials(BOOTSTRAP, SETTLEMENT)
            .await
            .unwrap();
        let (org, session) = store
            .bootstrap(
                BOOTSTRAP,
                "quota-root",
                "Quota root",
                PASSWORD,
                "Quota tenant",
            )
            .await
            .unwrap();
        let actor = store.authenticate_session(&session.token).await.unwrap();
        let resource = store
            .create_resource(&actor, org.id, ContentKind::Order, "quota-order")
            .await
            .unwrap();
        let permit = store
            .authorize_content(&actor, org.id, resource.id, ContentAction::Execute)
            .await
            .unwrap();
        Self {
            _directory: directory,
            pool,
            store,
            actor,
            org,
            resource,
            permit,
            expires_at: session.expires_at - 1,
        }
    }

    async fn quotas(&self, platform: QuotaAmounts, organization: QuotaAmounts) {
        let expected = |status: Result<QuotaStatus>| match status {
            Ok(value) => value.revision,
            Err(IdentityError::QuotaExceeded) => 0,
            other => panic!("invalid fixture quota state: {other:?}"),
        };
        let platform_revision = expected(
            self.store
                .quota_status(&self.actor, QuotaScope::Platform)
                .await,
        );
        let organization_revision = expected(
            self.store
                .quota_status(&self.actor, QuotaScope::Organization(self.org.id))
                .await,
        );
        self.store
            .configure_quota(
                &self.actor,
                QuotaScope::Platform,
                platform_revision,
                platform,
            )
            .await
            .unwrap();
        self.store
            .configure_quota(
                &self.actor,
                QuotaScope::Organization(self.org.id),
                organization_revision,
                organization,
            )
            .await
            .unwrap();
    }

    async fn count(&self, table: &str) -> i64 {
        let query = match table {
            "effects" => "SELECT COUNT(*) AS count FROM quota_product_effects",
            "reservations" => "SELECT COUNT(*) AS count FROM identity_reservations",
            "audit" => "SELECT COUNT(*) AS count FROM identity_audit",
            _ => panic!("fixed fixture tables only"),
        };
        sqlx::query(query)
            .fetch_one(&self.pool)
            .await
            .unwrap()
            .get("count")
    }

    async fn status(&self, scope: QuotaScope) -> QuotaStatus {
        self.store.quota_status(&self.actor, scope).await.unwrap()
    }
}

async fn product_effect(guard: &mut LockedIdentityQuota<'_, '_, '_>, key: &str, id: Uuid) {
    sqlx::query("INSERT INTO quota_product_effects(id,reservation_id) VALUES($1,$2)")
        .bind(key)
        .bind(id.to_string())
        .execute(&mut **guard.transaction().unwrap())
        .await
        .unwrap();
}

async fn rollback_and_replay_contract(f: &Fixture) {
    f.quotas(amounts(100, 1), amounts(100, 1)).await;
    let before = f.status(QuotaScope::Platform).await;
    let audit = f.count("audit").await;
    let current_org = f.store.organization(&f.actor, f.org.id).await.unwrap();
    let mut tx = f.pool.begin().await.unwrap();
    let transient = {
        let mut guard = f.store.lock_quota_in_transaction(&mut tx).await.unwrap();
        assert_eq!(
            guard
                .organization_in_transaction(&f.actor, f.org.id)
                .await
                .unwrap(),
            current_org
        );
        let permit = guard
            .authorize_content_in_transaction(
                &f.actor,
                f.org.id,
                f.resource.id,
                ContentAction::Execute,
            )
            .await
            .unwrap();
        let receipt = guard
            .reserve_quota_in_transaction(
                &f.actor,
                &permit,
                "same-intent",
                amounts(70, 1),
                f.expires_at,
            )
            .await
            .unwrap();
        product_effect(&mut guard, "rolled-back-effect", receipt.id).await;
        guard
            .validate_content_in_transaction(&permit)
            .await
            .unwrap();
        receipt
    };
    tx.rollback().await.unwrap();
    assert_eq!(f.status(QuotaScope::Platform).await, before);
    assert_eq!(f.count("reservations").await, 0);
    assert_eq!(f.count("effects").await, 0);
    assert_eq!(f.count("audit").await, audit);

    let mut tx = f.pool.begin().await.unwrap();
    let persisted = {
        let mut guard = f.store.lock_quota_in_transaction(&mut tx).await.unwrap();
        let receipt = guard
            .reserve_quota_in_transaction(
                &f.actor,
                &f.permit,
                "same-intent",
                amounts(70, 1),
                f.expires_at,
            )
            .await
            .unwrap();
        assert_ne!(receipt.id, transient.id);
        let replay = guard
            .reserve_quota_in_transaction(
                &f.actor,
                &f.permit,
                "same-intent",
                amounts(70, 1),
                f.expires_at,
            )
            .await
            .unwrap();
        assert_eq!(receipt, replay);
        assert!(matches!(
            guard
                .reserve_quota_in_transaction(
                    &f.actor,
                    &f.permit,
                    "same-intent",
                    amounts(69, 1),
                    f.expires_at
                )
                .await,
            Err(IdentityError::Conflict)
        ));
        product_effect(&mut guard, "persisted-effect", receipt.id).await;
        guard
            .validate_content_in_transaction(&f.permit)
            .await
            .unwrap();
        receipt
    };
    tx.commit().await.unwrap();
    assert_eq!(f.count("effects").await, 1);
    assert_eq!(f.count("reservations").await, 1);
    assert_eq!(f.status(QuotaScope::Platform).await.held, amounts(70, 1));
    assert_eq!(
        f.store
            .reserve_quota(
                &f.actor,
                &f.permit,
                "same-intent",
                amounts(70, 1),
                f.expires_at
            )
            .await
            .unwrap(),
        persisted
    );
}

#[tokio::test]
async fn reserve_receipt_quota_audit_and_product_row_share_caller_rollback() {
    let f = Fixture::sqlite().await;
    rollback_and_replay_contract(&f).await;
    f.pool.close().await;
}

async fn caught_failure_contract(f: &Fixture) {
    f.quotas(amounts(100, 1), amounts(10, 1)).await;
    let platform = f.status(QuotaScope::Platform).await;
    let tenant = f.status(QuotaScope::Organization(f.org.id)).await;
    let audit = f.count("audit").await;
    let mut tx = f.pool.begin().await.unwrap();
    {
        let mut guard = f.store.lock_quota_in_transaction(&mut tx).await.unwrap();
        assert!(matches!(
            guard
                .reserve_quota_in_transaction(
                    &f.actor,
                    &f.permit,
                    "tenant-exhausted",
                    amounts(70, 1),
                    f.expires_at
                )
                .await,
            Err(IdentityError::QuotaExceeded)
        ));
        // The caller deliberately catches the failure and commits unrelated
        // product work. Neither quota scope, audit nor receipt may leak through.
        product_effect(&mut guard, "independent-product-work", Uuid::new_v4()).await;
        guard
            .validate_content_in_transaction(&f.permit)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    assert_eq!(f.status(QuotaScope::Platform).await, platform);
    assert_eq!(f.status(QuotaScope::Organization(f.org.id)).await, tenant);
    assert_eq!(f.count("audit").await, audit);
    assert_eq!(f.count("reservations").await, 0);
    assert_eq!(f.count("effects").await, 1);
}

#[tokio::test]
async fn caught_tenant_quota_failure_rolls_back_platform_charge_before_outer_commit() {
    let f = Fixture::sqlite().await;
    caught_failure_contract(&f).await;
    f.pool.close().await;
}

async fn activation_and_settlement_contract(f: &Fixture) {
    let effects_before = f.count("effects").await;
    f.quotas(amounts(100, 1), amounts(100, 1)).await;
    let original = f
        .store
        .reserve_quota(
            &f.actor,
            &f.permit,
            "activation-intent",
            amounts(70, 1),
            f.expires_at,
        )
        .await
        .unwrap();
    let mut tx = f.pool.begin().await.unwrap();
    let provisional = {
        let mut guard = f.store.lock_quota_in_transaction(&mut tx).await.unwrap();
        let execution = guard
            .activate_quota_in_transaction(&f.actor, &original)
            .await
            .unwrap();
        product_effect(&mut guard, "activation-rollback", original.id).await;
        guard
            .validate_execution_in_transaction(&execution)
            .await
            .unwrap();
        execution
    };
    tx.rollback().await.unwrap();
    assert!(matches!(
        f.store.validate_execution(&provisional).await,
        Err(IdentityError::Conflict)
    ));
    assert_eq!(f.count("effects").await, effects_before);
    let committed = f.store.activate_quota(&f.actor, &original).await.unwrap();
    let held = f.status(QuotaScope::Platform).await;
    let evidence = "d".repeat(64);
    let mut tx = f.pool.begin().await.unwrap();
    {
        let mut guard = f.store.lock_quota_in_transaction(&mut tx).await.unwrap();
        guard
            .finalize_quota_in_transaction(
                SETTLEMENT,
                original.id,
                &original.request_sha256,
                amounts(20, 0),
                &evidence,
                false,
            )
            .await
            .unwrap();
        product_effect(&mut guard, "settlement-rollback", original.id).await;
    }
    tx.rollback().await.unwrap();
    f.store.validate_execution(&committed).await.unwrap();
    assert_eq!(f.status(QuotaScope::Platform).await, held);
    assert_eq!(f.count("effects").await, effects_before);

    let mut tx = f.pool.begin().await.unwrap();
    {
        let mut guard = f.store.lock_quota_in_transaction(&mut tx).await.unwrap();
        let final_receipt = guard
            .finalize_quota_in_transaction(
                SETTLEMENT,
                original.id,
                &original.request_sha256,
                amounts(20, 0),
                &evidence,
                false,
            )
            .await
            .unwrap();
        assert_eq!(
            final_receipt,
            guard
                .finalize_quota_in_transaction(
                    SETTLEMENT,
                    original.id,
                    &original.request_sha256,
                    amounts(20, 0),
                    &evidence,
                    false
                )
                .await
                .unwrap()
        );
        assert!(matches!(
            guard
                .finalize_quota_in_transaction(
                    SETTLEMENT,
                    original.id,
                    &original.request_sha256,
                    amounts(21, 0),
                    &evidence,
                    false
                )
                .await,
            Err(IdentityError::Conflict)
        ));
        product_effect(&mut guard, "settlement-committed", original.id).await;
    }
    tx.commit().await.unwrap();
    for scope in [QuotaScope::Platform, QuotaScope::Organization(f.org.id)] {
        let status = f.status(scope).await;
        assert_eq!(status.held, QuotaAmounts::default());
        assert_eq!(status.consumed, amounts(20, 0));
    }
    assert_eq!(f.count("effects").await, effects_before + 1);
    assert_eq!(
        f.store
            .reserve_quota(
                &f.actor,
                &f.permit,
                "activation-intent",
                amounts(70, 1),
                f.expires_at
            )
            .await
            .unwrap(),
        original
    );
    assert!(matches!(
        f.store.activate_quota(&f.actor, &original).await,
        Err(IdentityError::Conflict)
    ));
}

#[tokio::test]
async fn activation_and_executor_settlement_are_provisional_until_outer_commit() {
    let f = Fixture::sqlite().await;
    activation_and_settlement_contract(&f).await;
    f.pool.close().await;
}

async fn competing_tenant_contract(f: &Fixture) {
    let effects_before = f.count("effects").await;
    let reservations_before = f.count("reservations").await;
    f.quotas(amounts(100, 1), amounts(100, 1)).await;
    let other = f
        .store
        .create_organization(&f.actor, "Competing tenant", "other-tenant")
        .await
        .unwrap();
    f.store
        .configure_quota(
            &f.actor,
            QuotaScope::Organization(other.id),
            0,
            amounts(100, 1),
        )
        .await
        .unwrap();
    let resource = f
        .store
        .create_resource(&f.actor, other.id, ContentKind::Order, "other-order")
        .await
        .unwrap();
    let other_permit = f
        .store
        .authorize_content(&f.actor, other.id, resource.id, ContentAction::Execute)
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let contend = |permit: ContentPermit, key: &'static str| {
        let pool = f.pool.clone();
        let store = f.store.clone();
        let actor = f.actor.clone();
        let barrier = barrier.clone();
        let expires_at = f.expires_at;
        async move {
            let mut tx = pool.begin().await.unwrap();
            barrier.wait().await;
            let result = {
                let mut guard = store.lock_quota_in_transaction(&mut tx).await.unwrap();
                match guard
                    .reserve_quota_in_transaction(&actor, &permit, key, amounts(40, 1), expires_at)
                    .await
                {
                    Ok(receipt) => {
                        product_effect(&mut guard, key, receipt.id).await;
                        guard
                            .validate_content_in_transaction(&permit)
                            .await
                            .unwrap();
                        Ok(receipt)
                    }
                    Err(error) => Err(error),
                }
            };
            if result.is_ok() {
                tx.commit().await.unwrap();
            } else {
                tx.rollback().await.unwrap();
            }
            result
        }
    };
    let (first, second) = tokio::join!(
        contend(f.permit.clone(), "first"),
        contend(other_permit, "second")
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert!(matches!(
        if first.is_err() { first } else { second },
        Err(IdentityError::QuotaExceeded)
    ));
    assert_eq!(f.status(QuotaScope::Platform).await.held, amounts(40, 1));
    let one = f.status(QuotaScope::Organization(f.org.id)).await;
    let two = f.status(QuotaScope::Organization(other.id)).await;
    assert_eq!(one.held.model_tokens + two.held.model_tokens, 40);
    assert_eq!(one.held.concurrent_runs + two.held.concurrent_runs, 1);
    assert_eq!(f.count("effects").await, effects_before + 1);
    assert_eq!(f.count("reservations").await, reservations_before + 1);
}

#[tokio::test]
async fn competing_tenant_caller_transactions_share_the_global_run_slot() {
    let f = Fixture::sqlite().await;
    competing_tenant_contract(&f).await;
    f.pool.close().await;
}

async fn session_lock_wait_contract(f: &Fixture) {
    f.quotas(amounts(100, 1), amounts(100, 1)).await;
    let session = f.store.login("quota-root", PASSWORD, 1_000).await.unwrap();
    let actor = f.store.authenticate_session(&session.token).await.unwrap();
    let permit = f
        .store
        .authorize_content(&actor, f.org.id, f.resource.id, ContentAction::Execute)
        .await
        .unwrap();
    let mut owner_tx = f.pool.begin().await.unwrap();
    let _owner_guard = f
        .store
        .lock_quota_in_transaction(&mut owner_tx)
        .await
        .unwrap();
    let pool = f.pool.clone();
    let store = f.store.clone();
    let (ready, waiting) = tokio::sync::oneshot::channel();
    let contender = tokio::spawn(async move {
        let mut tx = pool.begin().await.unwrap();
        ready.send(()).unwrap();
        let result = {
            let mut guard = store.lock_quota_in_transaction(&mut tx).await.unwrap();
            guard
                .reserve_quota_in_transaction(
                    &actor,
                    &permit,
                    "expired-contender",
                    amounts(40, 1),
                    session.expires_at - 1,
                )
                .await
        };
        tx.rollback().await.unwrap();
        result
    });
    waiting.await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !contender.is_finished(),
        "contender must wait on the owned writer lock"
    );
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    owner_tx.commit().await.unwrap();
    assert!(matches!(
        contender.await.unwrap(),
        Err(IdentityError::Unauthenticated)
    ));
    assert_eq!(
        f.status(QuotaScope::Platform).await.held,
        QuotaAmounts::default()
    );
    assert_eq!(f.count("reservations").await, 0);
}

#[tokio::test]
async fn actual_installation_lock_wait_cannot_extend_the_original_session_expiry() {
    let f = Fixture::sqlite().await;
    session_lock_wait_contract(&f).await;
    f.pool.close().await;
}

#[tokio::test]
async fn failed_savepoint_rollback_poisons_every_guard_api() {
    let f = Fixture::sqlite().await;
    f.quotas(amounts(100, 1), amounts(100, 1)).await;
    // This trigger exists only in this owned fixture database. It deliberately
    // aborts the whole SQL transaction, making savepoint rollback impossible.
    sqlx::query("CREATE TRIGGER quota_abort_outer BEFORE INSERT ON identity_audit WHEN NEW.action = 'quota_reserved' BEGIN SELECT RAISE(ROLLBACK, 'synthetic outer rollback'); END")
        .execute(&f.pool).await.unwrap();
    let mut tx = f.pool.begin().await.unwrap();
    {
        let mut guard = f.store.lock_quota_in_transaction(&mut tx).await.unwrap();
        assert!(matches!(
            guard
                .reserve_quota_in_transaction(
                    &f.actor,
                    &f.permit,
                    "forced-rollback",
                    amounts(40, 1),
                    f.expires_at
                )
                .await,
            Err(IdentityError::Storage(_))
        ));
        assert!(matches!(
            guard.transaction(),
            Err(IdentityError::TransactionPoisoned)
        ));
        assert!(matches!(
            guard.organization_in_transaction(&f.actor, f.org.id).await,
            Err(IdentityError::TransactionPoisoned)
        ));
        assert!(matches!(
            guard.validate_content_in_transaction(&f.permit).await,
            Err(IdentityError::TransactionPoisoned)
        ));
        assert!(matches!(
            guard
                .reserve_quota_in_transaction(
                    &f.actor,
                    &f.permit,
                    "cannot-reuse",
                    amounts(1, 1),
                    f.expires_at
                )
                .await,
            Err(IdentityError::TransactionPoisoned)
        ));
    }
    // The trigger has already rolled it back. We still request rollback, never
    // commit. No reservation or product effect can survive the forced failure.
    let _ = tx.rollback().await;
    assert_eq!(f.count("reservations").await, 0);
    assert_eq!(f.count("effects").await, 0);
    f.pool.close().await;
}

async fn fresh_postgres() -> Fixture {
    let url = std::env::var("COWORK_IDENTITY_QUOTA_TX_TEST_POSTGRES_URL")
        .expect("fresh fixture URL required");
    assert!(url.starts_with("postgres:") || url.starts_with("postgresql:"));
    sqlx::any::install_default_drivers();
    let preflight = AnyPool::connect(&url).await.unwrap();
    let count: i64 = sqlx::query("SELECT COUNT(*) AS count FROM information_schema.tables WHERE table_schema = 'public' AND (table_name LIKE 'identity_%' OR table_name = 'quota_product_effects')")
        .fetch_one(&preflight).await.unwrap().get("count");
    assert_eq!(count, 0, "requires an unused owned fixture database");
    preflight.close().await;
    Fixture::new(&url, BackendKind::Postgres, None).await
}

#[tokio::test]
#[ignore = "requires a fresh private PostgreSQL database for caller quota transactions"]
async fn postgres_caller_transaction_quota_rollback_and_settlement() {
    let f = fresh_postgres().await;
    rollback_and_replay_contract(&f).await;
    // Use another tenant resource and reset only the existing policy via its CAS;
    // preserving all earlier history proves no schema/fixture reset is needed.
    let platform = f.status(QuotaScope::Platform).await;
    assert_eq!(platform.held, amounts(70, 1));
    let receipt: String = sqlx::query("SELECT payload FROM identity_reservations")
        .fetch_one(&f.pool)
        .await
        .unwrap()
        .get("payload");
    let reserved: QuotaReservation = serde_json::from_str(&receipt).unwrap();
    f.store
        .finalize_quota(
            SETTLEMENT,
            reserved.id,
            &reserved.request_sha256,
            QuotaAmounts::default(),
            &"f".repeat(64),
            true,
        )
        .await
        .unwrap();
    assert_eq!(
        f.status(QuotaScope::Platform).await.held,
        QuotaAmounts::default()
    );
    activation_and_settlement_contract(&f).await;
    f.pool.close().await;
}

#[tokio::test]
#[ignore = "requires a fresh private PostgreSQL database for caller quota transactions"]
async fn postgres_caught_error_and_competing_tenant_transactions() {
    let f = fresh_postgres().await;
    caught_failure_contract(&f).await;
    competing_tenant_contract(&f).await;
    f.pool.close().await;
}

#[tokio::test]
#[ignore = "requires a fresh private PostgreSQL database for caller quota transactions"]
async fn postgres_actual_installation_lock_wait_rechecks_session_expiry() {
    let f = fresh_postgres().await;
    session_lock_wait_contract(&f).await;

    // Unlike SQLite's one-writer database, PostgreSQL permits a distinct product
    // row to be held before the identity installation lock is acquired. This
    // exercises the required second authority check after the *later* row wait.
    let initial_product_id = Uuid::new_v4();
    sqlx::query("INSERT INTO quota_product_effects(id,reservation_id) VALUES('later-lock',$1)")
        .bind(initial_product_id.to_string())
        .execute(&f.pool)
        .await
        .unwrap();
    let session = f.store.login("quota-root", PASSWORD, 1_000).await.unwrap();
    let actor = f.store.authenticate_session(&session.token).await.unwrap();
    let permit = f
        .store
        .authorize_content(&actor, f.org.id, f.resource.id, ContentAction::Execute)
        .await
        .unwrap();
    let reservation = f
        .store
        .reserve_quota(
            &actor,
            &permit,
            "later-product-lock",
            amounts(40, 1),
            session.expires_at - 1,
        )
        .await
        .unwrap();
    let mut blocking_tx = f.pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE quota_product_effects SET reservation_id=reservation_id WHERE id='later-lock'",
    )
    .execute(&mut *blocking_tx)
    .await
    .unwrap();
    let mut tx = f.pool.begin().await.unwrap();
    {
        let mut guard = f.store.lock_quota_in_transaction(&mut tx).await.unwrap();
        let execution = guard
            .activate_quota_in_transaction(&actor, &reservation)
            .await
            .unwrap();
        guard
            .validate_execution_in_transaction(&execution)
            .await
            .unwrap();
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1_100)).await;
            blocking_tx.rollback().await.unwrap();
        });
        let before_wait = tokio::time::Instant::now();
        sqlx::query("UPDATE quota_product_effects SET reservation_id=$1 WHERE id='later-lock'")
            .bind(reservation.id.to_string())
            .execute(&mut **guard.transaction().unwrap())
            .await
            .unwrap();
        assert!(before_wait.elapsed() >= Duration::from_millis(1_000));
        release.await.unwrap();
        assert!(matches!(
            guard.validate_execution_in_transaction(&execution).await,
            Err(IdentityError::Unauthenticated)
        ));
    }
    tx.rollback().await.unwrap();
    let persisted: String =
        sqlx::query("SELECT reservation_id FROM quota_product_effects WHERE id='later-lock'")
            .fetch_one(&f.pool)
            .await
            .unwrap()
            .get("reservation_id");
    assert_eq!(persisted, initial_product_id.to_string());
    assert_eq!(f.status(QuotaScope::Platform).await.held, amounts(40, 1));
    let payload: String = sqlx::query("SELECT payload FROM identity_reservations WHERE id=$1")
        .bind(reservation.id.to_string())
        .fetch_one(&f.pool)
        .await
        .unwrap()
        .get("payload");
    let original: QuotaReservation = serde_json::from_str(&payload).unwrap();
    assert_eq!(
        original, reservation,
        "failed late admission retains original prepared charge"
    );
    f.pool.close().await;
}
