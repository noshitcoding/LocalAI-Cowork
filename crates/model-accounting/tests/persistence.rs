use cowork_model_accounting::money::*;
use cowork_model_accounting::persistence::*;
use cowork_model_accounting::{Error, digest, sha256};
use sqlx::any::AnyPoolOptions;
use sqlx::{AnyPool, Row};
use std::sync::Arc;
use tokio::sync::Barrier;

fn price() -> PriceVersion {
    PriceVersion {
        id: "fixture-price".into(),
        version: 1,
        provider_id: "fixture".into(),
        model_sha256: sha256(b"fixture-model"),
        currency: Currency {
            code: "USD".into(),
            scale: 6,
        },
        input_units_per_million: 1_000_000,
        output_units_per_million: 2_000_000,
        cached_input_units_per_million: None,
        source_sha256: sha256(b"fixture-immutable-price"),
    }
}

async fn setup(pool: &AnyPool) {
    let mut tx = pool.begin().await.unwrap();
    migrate(&mut tx).await.unwrap();
    sqlx::query("CREATE TABLE accounting_test_run_effects(id TEXT PRIMARY KEY, request_sha256 TEXT NOT NULL)").execute(&mut *tx).await.unwrap();
    let mut locked = load_locked(&mut tx).await.unwrap();
    locked
        .ledger
        .register_price(locked.ledger.revision(), price())
        .unwrap();
    for (scope, limit) in [
        (PoolScope::Run("run-a".into()), 100),
        (PoolScope::Run("run-b".into()), 100),
        (PoolScope::Organization("org".into()), 20),
        (PoolScope::Instance("instance".into()), 100),
    ] {
        locked
            .ledger
            .activate_pool(
                locked.ledger.revision(),
                PoolPolicy {
                    scope,
                    version: 1,
                    currency: price().currency,
                    limit_units: limit,
                },
            )
            .unwrap();
    }
    locked.persist(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
}

fn spec(id: &str) -> ReservationSpec {
    let mut pools = vec![
        PoolBinding {
            scope: PoolScope::Run(id.into()),
            version: 1,
        },
        PoolBinding {
            scope: PoolScope::Organization("org".into()),
            version: 1,
        },
        PoolBinding {
            scope: PoolScope::Instance("instance".into()),
            version: 1,
        },
    ];
    pools.sort();
    ReservationSpec {
        attempt_id: id.into(),
        request_sha256: sha256(id.as_bytes()),
        provider_id: "fixture".into(),
        model_sha256: sha256(b"fixture-model"),
        price_id: "fixture-price".into(),
        price_version: 1,
        input_upper_bound: 10,
        output_upper_bound: 5,
        pools,
    }
}

async fn admit_in_tx(pool: &AnyPool, id: &str, commit: bool) -> Result<AccountingOperation, Error> {
    let mut tx = pool.begin().await.unwrap();
    let mut locked = load_locked(&mut tx).await?;
    let specification = spec(id);
    let required: Vec<_> = specification
        .pools
        .iter()
        .map(|p| p.scope.clone())
        .collect();
    let admission =
        locked
            .ledger
            .admit(locked.ledger.revision(), specification.clone(), &required)?;
    let value = AccountingOperation {
        id: id.into(),
        kind: AccountingOperationKind::Admission,
        request_sha256: digest(&specification)?,
        accepted_ledger_revision: admission.receipt.accepted_revision,
        receipt: serde_json::to_value(admission.receipt).unwrap(),
    };
    locked.persist(&mut tx).await?;
    let new = record_operation(&mut tx, &value).await?;
    assert_eq!(new, admission.newly_reserved);
    if new {
        // Represents the caller's real model-attempt/run/event write in this
        // very same transaction; no separate accounting transaction exists.
        sqlx::query("INSERT INTO accounting_test_run_effects(id,request_sha256) VALUES($1,$2)")
            .bind(id)
            .bind(&specification.request_sha256)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    if commit {
        tx.commit().await.unwrap();
    } else {
        tx.rollback().await.unwrap();
    }
    Ok(value)
}

async fn contract(pool: &AnyPool) {
    setup(pool).await;
    let mut tx = pool.begin().await.unwrap();
    let baseline = load_locked(&mut tx).await.unwrap().ledger;
    tx.rollback().await.unwrap();
    let rolled_back = admit_in_tx(pool, "run-a", false).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(load_locked(&mut tx).await.unwrap().ledger, baseline);
    assert!(operation(&mut tx, "run-a").await.unwrap().is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM accounting_test_run_effects")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        0
    );
    tx.rollback().await.unwrap();

    let barrier = Arc::new(Barrier::new(3));
    let mut tasks = Vec::new();
    for id in ["run-a", "run-b"] {
        let pool = pool.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            admit_in_tx(&pool, id, true).await
        }));
    }
    barrier.wait().await;
    let results = vec![
        tasks.remove(0).await.unwrap(),
        tasks.remove(0).await.unwrap(),
    ];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::BudgetExceeded)))
            .count(),
        1
    );
    let winner = results.into_iter().find_map(|r| r.ok()).unwrap();
    let replay = admit_in_tx(pool, &winner.id, true).await.unwrap();
    assert_eq!(replay, winner);
    if winner.id == rolled_back.id {
        assert_eq!(winner, rolled_back);
    }
    let mut tx = pool.begin().await.unwrap();
    let locked = load_locked(&mut tx).await.unwrap();
    assert_eq!(locked.ledger.reservations().len(), 1);
    assert_eq!(
        locked
            .ledger
            .charged_to(&PoolScope::Organization("org".into()))
            .unwrap(),
        20
    );
    assert_eq!(
        operation(&mut tx, &winner.id).await.unwrap().unwrap(),
        winner
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM accounting_test_run_effects")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        1
    );
    let mut forged = winner.clone();
    forged.request_sha256 = sha256(b"different request");
    assert_eq!(
        record_operation(&mut tx, &forged).await,
        Err(Error::Conflict)
    );
    tx.rollback().await.unwrap();
}

async fn sqlite_pool() -> (tempfile::TempDir, AnyPool) {
    sqlx::any::install_default_drivers();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("accounting.sqlite");
    let url = format!("sqlite:{}?mode=rwc", path.to_string_lossy());
    let pool = AnyPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .unwrap();
    (directory, pool)
}

#[tokio::test]
async fn sqlite_actual_transaction_rollback_global_scope_competition_and_replay() {
    let (_directory, pool) = sqlite_pool().await;
    contract(&pool).await;
    pool.close().await;
}

#[tokio::test]
async fn sqlite_payload_hash_header_and_unknown_format_fail_closed() {
    let (_directory, pool) = sqlite_pool().await;
    setup(&pool).await;
    let row = sqlx::query("SELECT payload,payload_sha256 FROM accounting_ledgers WHERE id=$1")
        .bind(LEDGER_ID)
        .fetch_one(&pool)
        .await
        .unwrap();
    let payload: String = row.try_get("payload").unwrap();
    for statement in [
        "UPDATE accounting_ledgers SET payload_sha256='forged'",
        "UPDATE accounting_ledgers SET format_version=99",
        "UPDATE accounting_ledgers SET revision=revision+1",
    ] {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
        assert!(matches!(load_locked(&mut tx).await, Err(Error::Conflict)));
        tx.rollback().await.unwrap();
    }
    let actual: String = sqlx::query_scalar("SELECT payload FROM accounting_ledgers WHERE id=$1")
        .bind(LEDGER_ID)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(actual, payload);
    pool.close().await;
}

#[tokio::test]
#[ignore = "Requires an explicitly dedicated disposable COWORK_TEST_ACCOUNTING_POSTGRES database"]
async fn postgres_actual_transaction_rollback_global_scope_competition_and_replay() {
    sqlx::any::install_default_drivers();
    let url = std::env::var("COWORK_TEST_ACCOUNTING_POSTGRES").unwrap();
    let pool = AnyPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .unwrap();
    contract(&pool).await;
    pool.close().await;
}
