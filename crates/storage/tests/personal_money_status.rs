//! Synthetic operator/ledger fixtures for the read-only status, not model IO.
use cowork_contracts::{PersonalMoneyStatus, ProviderKind, ProviderProfile};
use cowork_model_accounting::{persistence, sha256};
use cowork_storage::{Store, model_money::*};
use serde_json::{Value, json};
use sqlx::{AnyPool, Row, any::AnyPoolOptions};
use std::collections::BTreeMap;

struct Fixture {
    store: Store,
    pool: AnyPool,
    temp: Option<tempfile::TempDir>,
    pg: Option<(String, String)>,
}
impl Fixture {
    async fn new() -> Self {
        let (url, temp, pg) = match std::env::var("COWORK_PRIVATE_MONEY_STATUS_PG_URL") {
            Ok(base) => {
                let marker = std::env::var("COWORK_PRIVATE_MONEY_STATUS_FIXTURE")
                    .expect("private fixture marker");
                assert_eq!(marker.len(), 32);
                assert!(marker.bytes().all(|b| b.is_ascii_hexdigit()));
                let parsed = reqwest::Url::parse(&base).unwrap();
                assert_eq!(parsed.host_str(), Some("127.0.0.1"));
                assert!(parsed.port().is_some_and(|port| port != 5432));
                assert!(parsed.query().is_none());
                let schema = format!("money_status_{}", cowork_storage::id().replace('-', ""));
                let admin = sqlx::PgPool::connect(&base).await.unwrap();
                sqlx::query(&format!("CREATE SCHEMA {schema}"))
                    .execute(&admin)
                    .await
                    .unwrap();
                admin.close().await;
                (
                    format!("{base}?options=-csearch_path%3D{schema}"),
                    None,
                    Some((base, schema)),
                )
            }
            Err(_) => {
                let temp = tempfile::tempdir().unwrap();
                let path = temp.path().join("status.sqlite");
                let encoded = percent_encoding::utf8_percent_encode(
                    path.to_str().unwrap(),
                    percent_encoding::NON_ALPHANUMERIC,
                );
                (format!("sqlite:{encoded}?mode=rwc"), Some(temp), None)
            }
        };
        let store = Store::connect(&url).await.unwrap();
        let pool = AnyPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .unwrap();
        Self {
            store,
            pool,
            temp,
            pg,
        }
    }
    async fn close(self) {
        self.store.close().await;
        self.pool.close().await;
        drop(self.store);
        drop(self.pool);
        if let Some((base, schema)) = self.pg {
            assert!(
                schema.starts_with("money_status_")
                    && schema
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            );
            let admin = sqlx::PgPool::connect(&base).await.unwrap();
            sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
                .execute(&admin)
                .await
                .unwrap();
            admin.close().await;
        }
        if let Some(temp) = self.temp {
            // SQLx closes the pool before its SQLite worker thread has dropped
            // the native file handle. Keep only this fixture's exact directory
            // until that asynchronous close has completed on Windows.
            let path = temp.keep();
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                match std::fs::remove_dir_all(&path) {
                    Ok(()) => break,
                    Err(error)
                        if cfg!(windows)
                            && error.raw_os_error() == Some(32)
                            && tokio::time::Instant::now() < deadline =>
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                    Err(error) => panic!("own fixture cleanup {}: {error}", path.display()),
                }
            }
        }
    }
    async fn edit_ledger(&self, edit: impl FnOnce(&mut MonetaryLedger)) {
        let mut tx = self.pool.begin().await.unwrap();
        let mut locked = persistence::load_locked(&mut tx).await.unwrap();
        edit(&mut locked.ledger);
        locked.persist(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    async fn status(&self) -> Value {
        serde_json::to_value(self.store.personal_money_status().await.unwrap()).unwrap()
    }
    async fn replace_payload(&self, table: &str, id: &str, payload: &Value) {
        assert!(matches!(
            table,
            "accounting_personal_installation" | "accounting_personal_configuration"
        ));
        let payload = serde_json::to_string(payload).unwrap();
        sqlx::query(&format!(
            "UPDATE {table} SET payload=$1,payload_sha256=$2 WHERE id=$3"
        ))
        .bind(&payload)
        .bind(sha256(payload.as_bytes()))
        .bind(id)
        .execute(&self.pool)
        .await
        .unwrap();
    }
    async fn all_rows(&self) -> BTreeMap<String, Vec<Vec<Option<String>>>> {
        let tables: Vec<String> = if self.pg.is_some() {
            sqlx::query_scalar("SELECT CAST(table_name AS TEXT) FROM information_schema.tables WHERE table_schema=current_schema() AND table_type='BASE TABLE'").fetch_all(&self.pool).await.unwrap()
        } else {
            sqlx::query_scalar(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )
            .fetch_all(&self.pool)
            .await
            .unwrap()
        };
        let mut result = BTreeMap::new();
        for table in tables {
            assert!(
                table
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            );
            let columns: Vec<String> = if self.pg.is_some() {
                sqlx::query_scalar("SELECT CAST(column_name AS TEXT) FROM information_schema.columns WHERE table_schema=current_schema() AND table_name=$1 ORDER BY ordinal_position").bind(&table).fetch_all(&self.pool).await.unwrap()
            } else {
                sqlx::query(&format!("PRAGMA table_info(\"{table}\")"))
                    .fetch_all(&self.pool)
                    .await
                    .unwrap()
                    .iter()
                    .map(|r| r.get("name"))
                    .collect()
            };
            assert!(
                columns
                    .iter()
                    .all(|name| name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            );
            let expressions = columns
                .iter()
                .map(|name| format!("CAST(\"{name}\" AS TEXT) AS \"{name}\""))
                .collect::<Vec<_>>()
                .join(",");
            let mut rows = sqlx::query(&format!("SELECT {expressions} FROM \"{table}\""))
                .fetch_all(&self.pool)
                .await
                .unwrap()
                .iter()
                .map(|row| {
                    columns
                        .iter()
                        .map(|name| row.get::<Option<String>, _>(name.as_str()))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            rows.sort();
            result.insert(table, rows);
        }
        result
    }
}

fn config(limit: u64, rate: u64) -> PersonalMoneyConfiguration {
    let currency = Currency {
        code: "USD".into(),
        scale: 6,
    };
    let provider = ProviderProfile {
        generation: Default::default(),
        id: "private-status-provider".into(),
        name: "Private synthetic status fixture".into(),
        kind: ProviderKind::Ollama,
        endpoint: "http://127.0.0.1:9".into(),
        model: "synthetic".into(),
        revision: 1,
        context_tokens: 1024,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    let price = PriceVersion {
        id: "private-status-price".into(),
        version: 1,
        provider_id: provider.id.clone(),
        model_sha256: sha256(b"synthetic-model"),
        currency: currency.clone(),
        input_units_per_million: rate,
        output_units_per_million: rate,
        cached_input_units_per_million: None,
        source_sha256: sha256(b"synthetic-price"),
    };
    PersonalMoneyConfiguration {
        format_version: 1,
        installation: PoolPolicy {
            scope: PoolScope::Installation("personal-status-fixture".into()),
            version: 1,
            currency,
            limit_units: limit,
        },
        project_limit_units: u64::MAX,
        run_limit_units: 9_007_199_254_740_993,
        pool_policy_version: 1,
        deployments: vec![PersonalMoneyDeployment {
            deployment: OperatorModelDeployment {
                id: "private-status-deployment".into(),
                provider,
                model_sha256: price.model_sha256.clone(),
                price_id: price.id.clone(),
                price_version: 1,
                token_mode: MonetaryTokenMode::ConservativeContextUpperBound,
            },
            price,
        }],
    }
}
fn admit(
    ledger: &mut MonetaryLedger,
    config: &PersonalMoneyConfiguration,
    id: &str,
    scopes: Vec<PoolScope>,
    input: u64,
    output: u64,
) {
    let price = &config.deployments[0].price;
    let run = PoolScope::Run(format!("run-{id}"));
    ledger
        .activate_pool(
            ledger.revision(),
            PoolPolicy {
                scope: run.clone(),
                version: 1,
                currency: price.currency.clone(),
                limit_units: u64::MAX,
            },
        )
        .unwrap();
    let mut scopes = scopes;
    scopes.push(run);
    let mut pools = scopes
        .iter()
        .map(|scope| ledger.active_binding(scope).unwrap())
        .collect::<Vec<_>>();
    pools.sort();
    ledger
        .admit(
            ledger.revision(),
            ReservationSpec {
                attempt_id: id.into(),
                request_sha256: sha256(id.as_bytes()),
                provider_id: price.provider_id.clone(),
                model_sha256: price.model_sha256.clone(),
                price_id: price.id.clone(),
                price_version: price.version,
                input_upper_bound: input,
                output_upper_bound: output,
                pools,
            },
            &scopes,
        )
        .unwrap();
}
fn complete(input: u64, output: u64) -> ProviderUsage {
    ProviderUsage {
        input_tokens: Some(input),
        output_tokens: Some(output),
        cached_input_tokens: Some(0),
        provider_receipt_sha256: Some(sha256(b"private-received-synthetic-evidence")),
    }
}

#[tokio::test]
async fn origins_and_exact_caps_persist_and_get_changes_no_durable_row() {
    let f = Fixture::new().await;
    assert_eq!(
        f.store.personal_money_status().await.unwrap(),
        PersonalMoneyStatus::NotConfigured {}
    );
    let c = config(u64::MAX, 1_000_000);
    f.store
        .register_personal_model_deployment(
            c.installation.clone(),
            c.deployments[0].deployment.clone(),
            c.deployments[0].price.clone(),
        )
        .await
        .unwrap();
    assert_eq!(f.status().await["origin"], "legacy_manual");
    f.store.configure_personal_money(&c).await.unwrap();
    let before = f.all_rows().await;
    let expected = f.status().await;
    assert_eq!(expected["origin"], "operator_configuration");
    assert_eq!(
        expected["installation"]["limit_units"],
        u64::MAX.to_string()
    );
    assert_eq!(expected["defaults"]["run_limit_units"], "9007199254740993");
    assert!(expected.get("ledger_revision").is_none());
    for _ in 0..5 {
        assert_eq!(f.status().await, expected);
    }
    assert_eq!(f.all_rows().await, before);
    f.close().await;
}

#[tokio::test]
async fn only_complete_settlement_is_confirmed_and_foreign_scopes_are_excluded() {
    let f = Fixture::new().await;
    let c = config(50, 1_000_000);
    f.store.configure_personal_money(&c).await.unwrap();
    f.edit_ledger(|ledger| {
        let own = vec![c.installation.scope.clone()];
        admit(ledger, &c, "confirmed", own.clone(), 10, 5);
        ledger
            .settle(ledger.revision(), "confirmed", complete(3, 2))
            .unwrap();
        admit(ledger, &c, "pending", own.clone(), 10, 5);
        admit(ledger, &c, "incomplete-settled", own.clone(), 10, 5);
        ledger
            .settle(
                ledger.revision(),
                "incomplete-settled",
                ProviderUsage {
                    input_tokens: Some(2),
                    output_tokens: None,
                    cached_input_tokens: None,
                    provider_receipt_sha256: None,
                },
            )
            .unwrap();
        admit(ledger, &c, "partial-unknown", own, 10, 5);
        ledger
            .uncertain_with_usage(
                ledger.revision(),
                "partial-unknown",
                "private truncated fixture",
                Some(ProviderUsage {
                    input_tokens: Some(20),
                    output_tokens: None,
                    cached_input_tokens: None,
                    provider_receipt_sha256: None,
                }),
            )
            .unwrap();
        let foreign = vec![
            PoolScope::Installation("foreign-installation".into()),
            PoolScope::Organization("foreign-organization".into()),
            PoolScope::Instance("foreign-instance".into()),
            PoolScope::OfflineDevice("foreign-device".into()),
            PoolScope::Project("foreign-project".into()),
        ];
        for scope in &foreign {
            ledger
                .activate_pool(
                    ledger.revision(),
                    PoolPolicy {
                        scope: scope.clone(),
                        version: 1,
                        currency: c.installation.currency.clone(),
                        limit_units: u64::MAX,
                    },
                )
                .unwrap();
        }
        admit(ledger, &c, "foreign", foreign, 10, 5);
    })
    .await;
    let pool = f.status().await["installation"].clone();
    for (name, value) in [
        ("confirmed_units", "5"),
        ("pending_units", "15"),
        ("unknown_units", "35"),
        ("charged_units", "55"),
        ("remaining_units", "0"),
        ("confirmed_attempts", "1"),
        ("pending_attempts", "1"),
        ("unknown_attempts", "2"),
    ] {
        assert_eq!(pool[name], value, "{name}");
    }
    assert_eq!(pool["overrun"], true);
    assert_eq!(pool["limit_exceeded"], true);
    assert_eq!(pool["no_headroom"], true);
    let serialized = f.status().await.to_string();
    for private in [
        "foreign-",
        "private-status-provider",
        "synthetic",
        "endpoint",
        "sha256",
        "ledger_revision",
        "credential",
    ] {
        assert!(!serialized.contains(private));
    }
    f.close().await;
}

#[tokio::test]
async fn zero_price_pending_unknown_and_exact_zero_headroom_stay_visible() {
    let f = Fixture::new().await;
    let c = config(0, 0);
    f.store.configure_personal_money(&c).await.unwrap();
    f.edit_ledger(|ledger| {
        admit(
            ledger,
            &c,
            "zero-pending",
            vec![c.installation.scope.clone()],
            10,
            5,
        );
        admit(
            ledger,
            &c,
            "zero-unknown",
            vec![c.installation.scope.clone()],
            10,
            5,
        );
        ledger
            .uncertain(
                ledger.revision(),
                "zero-unknown",
                "private unknown zero-price fixture",
            )
            .unwrap();
    })
    .await;
    let pool = f.status().await["installation"].clone();
    for name in [
        "charged_units",
        "confirmed_units",
        "pending_units",
        "unknown_units",
        "remaining_units",
    ] {
        assert_eq!(pool[name], "0");
    }
    assert_eq!(pool["pending_attempts"], "1");
    assert_eq!(pool["unknown_attempts"], "1");
    assert_eq!(pool["no_headroom"], true);
    assert_eq!(pool["limit_exceeded"], false);
    f.close().await;
}

#[tokio::test]
async fn old_policy_version_holds_still_charge_the_designated_scope() {
    let f = Fixture::new().await;
    let mut c = config(100, 1_000_000);
    f.store.configure_personal_money(&c).await.unwrap();
    f.edit_ledger(|ledger| {
        admit(
            ledger,
            &c,
            "old-policy-hold",
            vec![c.installation.scope.clone()],
            10,
            5,
        )
    })
    .await;
    c.installation.version = 2;
    f.edit_ledger(|ledger| {
        ledger
            .activate_pool(ledger.revision(), c.installation.clone())
            .unwrap();
    })
    .await;
    // Explicit private historical-policy fixture, not an operator hot-reload API.
    f.replace_payload(
        "accounting_personal_installation",
        "personal-installation-v1",
        &json!(c.installation),
    )
    .await;
    f.replace_payload(
        "accounting_personal_configuration",
        "personal-money-configuration-v1",
        &json!(c),
    )
    .await;
    let pool = f.status().await["installation"].clone();
    assert_eq!(pool["policy_version"], "2");
    assert_eq!(pool["pending_units"], "15");
    assert_eq!(pool["remaining_units"], "85");
    f.close().await;
}

#[tokio::test]
async fn invalid_hash_strict_json_and_inconsistent_activation_never_look_unconfigured() {
    let f = Fixture::new().await;
    let c = config(100, 1_000_000);
    f.store.configure_personal_money(&c).await.unwrap();
    let before = f.all_rows().await;
    let mut original_payloads = Vec::new();
    for (table, id, original) in [
        (
            "accounting_personal_installation",
            "personal-installation-v1",
            json!(c.installation),
        ),
        (
            "accounting_personal_configuration",
            "personal-money-configuration-v1",
            json!(c),
        ),
    ] {
        let payload: String =
            sqlx::query_scalar(&format!("SELECT payload FROM {table} WHERE id=$1"))
                .bind(id)
                .fetch_one(&f.pool)
                .await
                .unwrap();
        original_payloads.push((table, id, payload));
        sqlx::query(&format!(
            "UPDATE {table} SET payload_sha256='broken' WHERE id=$1"
        ))
        .bind(id)
        .execute(&f.pool)
        .await
        .unwrap();
        assert!(f.store.personal_money_status().await.is_err());
        f.replace_payload(table, id, &original).await;
        let mut unknown = original.clone();
        unknown["unrecognized_private_key"] = json!(true);
        f.replace_payload(table, id, &unknown).await;
        assert!(f.store.personal_money_status().await.is_err());
        f.replace_payload(table, id, &original).await;
    }
    for (key, value) in [
        ("version", json!(2)),
        ("limit_units", json!(99)),
        ("currency", json!({"code":"EUR","scale":6})),
        (
            "scope",
            json!({"kind":"installation","id":"wrong-personal"}),
        ),
    ] {
        let mut changed = json!(c.installation);
        changed[key] = value;
        f.replace_payload(
            "accounting_personal_installation",
            "personal-installation-v1",
            &changed,
        )
        .await;
        assert!(f.store.personal_money_status().await.is_err(), "{key}");
        f.replace_payload(
            "accounting_personal_installation",
            "personal-installation-v1",
            &json!(c.installation),
        )
        .await;
    }
    let mut changed = json!(c);
    changed["installation"]["limit_units"] = json!(99);
    f.replace_payload(
        "accounting_personal_configuration",
        "personal-money-configuration-v1",
        &changed,
    )
    .await;
    assert!(f.store.personal_money_status().await.is_err());
    f.replace_payload(
        "accounting_personal_configuration",
        "personal-money-configuration-v1",
        &json!(c),
    )
    .await;
    // Restore the exact initial typed serialization, not a sorted Value
    // serialization introduced deliberately by corruption fixtures above.
    for (table, id, payload) in original_payloads {
        sqlx::query(&format!(
            "UPDATE {table} SET payload=$1,payload_sha256=$2 WHERE id=$3"
        ))
        .bind(&payload)
        .bind(sha256(payload.as_bytes()))
        .bind(id)
        .execute(&f.pool)
        .await
        .unwrap();
    }
    assert_eq!(f.all_rows().await, before);
    f.close().await;
}

#[tokio::test]
async fn orphan_activation_missing_ledger_and_wrong_ledger_revision_fail_closed() {
    let f = Fixture::new().await;
    let c = config(100, 1_000_000);
    f.store.configure_personal_money(&c).await.unwrap();
    sqlx::query("DELETE FROM accounting_personal_configuration")
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(f.status().await["origin"], "legacy_manual");
    sqlx::query("DELETE FROM accounting_personal_installation")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(f.store.personal_money_status().await.is_err());
    sqlx::query("UPDATE accounting_ledgers SET revision=revision+1")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(f.store.personal_money_status().await.is_err());
    sqlx::query("DELETE FROM accounting_ledgers")
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(f.store.personal_money_status().await.is_err());
    f.close().await;
}

#[tokio::test]
async fn checked_aggregate_overflow_is_an_error_not_saturated_exposure() {
    let f = Fixture::new().await;
    let c = config(u64::MAX, 1_000_000);
    f.store.configure_personal_money(&c).await.unwrap();
    f.edit_ledger(|ledger| {
        for id in ["overflow-a", "overflow-b"] {
            admit(ledger, &c, id, vec![c.installation.scope.clone()], 1, 0);
        }
        for id in ["overflow-a", "overflow-b"] {
            ledger
                .settle(ledger.revision(), id, complete(u64::MAX, 0))
                .unwrap();
        }
    })
    .await;
    assert!(f.store.personal_money_status().await.is_err());
    f.close().await;
}

#[tokio::test]
async fn get_is_a_snapshot_even_while_ledger_writer_lock_is_held() {
    let f = Fixture::new().await;
    let mut tx = f.pool.begin().await.unwrap();
    let mut locked = persistence::load_locked(&mut tx).await.unwrap();
    let c = config(100, 1_000_000);
    locked
        .ledger
        .register_price(locked.ledger.revision(), c.deployments[0].price.clone())
        .unwrap();
    locked
        .ledger
        .activate_pool(locked.ledger.revision(), c.installation.clone())
        .unwrap();
    locked.persist(&mut tx).await.unwrap();
    // The atomic activation is not committed. A GET must read the old complete
    // state immediately, rather than execute a global noop UPDATE/read lock.
    for _ in 0..5 {
        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                f.store.personal_money_status()
            )
            .await
            .unwrap()
            .unwrap(),
            PersonalMoneyStatus::NotConfigured {}
        );
    }
    tx.rollback().await.unwrap();
    f.close().await;
}

#[tokio::test]
async fn concurrent_activation_never_mixes_config_installation_and_ledger() {
    let f = Fixture::new().await;
    let c = config(100, 1_000_000);
    let writer = f.store.clone();
    let reader = f.store.clone();
    let c2 = c.clone();
    let (write, reads) = tokio::join!(writer.configure_personal_money(&c2), async {
        let mut values = Vec::new();
        for _ in 0..100 {
            values.push(reader.personal_money_status().await.unwrap());
            tokio::task::yield_now().await;
        }
        values
    });
    write.unwrap();
    let after = f.store.personal_money_status().await.unwrap();
    for value in reads {
        assert!(value == (PersonalMoneyStatus::NotConfigured {}) || value == after);
    }
    assert!(matches!(
        after,
        PersonalMoneyStatus::OperatorConfiguration { .. }
    ));
    f.close().await;
}
