//! Original private-server renew must obey the shared Identity UTC fence.
//! Synthetic SQLite only; no model, tool, runner, service or native start.
use cowork_domain::Error;
use cowork_identity::IdentityError;
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{Store, android_personal_provider};
use serde_json::{Value, json};
use sqlx::{AnyPool, Row, any::AnyPoolOptions};
use std::collections::BTreeMap;
use uuid::Uuid;
use zeroize::Zeroizing;

struct Keys;
impl KeyStore for Keys {
    fn load(&self, _: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        Ok(Zeroizing::new([37; 32]))
    }
}
fn id(value: &Value, key: &str) -> Uuid {
    value[key].as_str().unwrap().parse().unwrap()
}
type ProductState = BTreeMap<String, Vec<Vec<String>>>;
async fn product_state(pool: &AnyPool) -> ProductState {
    let mut state = BTreeMap::new();
    for table in [
        "android_private_server_runs",
        "android_private_server_events",
        "android_private_loop_checkpoints",
        "android_private_loop_steps",
        "android_private_loop_versions",
        "android_private_loop_entries",
        "android_private_loop_requests",
        "android_private_loop_artifacts",
        "android_private_loop_results",
        "android_private_workflow_runs",
        "android_private_workflow_nodes",
        "android_private_snapshots",
        "android_private_snapshot_chunks",
        "android_private_files",
    ] {
        let columns = sqlx::query(&format!("PRAGMA table_info(\"{table}\")"))
            .fetch_all(pool)
            .await
            .unwrap();
        assert!(
            !columns.is_empty(),
            "Missing original product table {table}"
        );
        let projection = columns
            .iter()
            .map(|column| {
                let name: String = column.try_get("name").unwrap();
                format!("quote(\"{}\")", name.replace('"', "\"\""))
            })
            .collect::<Vec<_>>()
            .join(",");
        let rows = sqlx::query(&format!(
            "SELECT {projection} FROM \"{table}\" ORDER BY rowid"
        ))
        .fetch_all(pool)
        .await
        .unwrap();
        let values = rows
            .iter()
            .map(|row| {
                (0..columns.len())
                    .map(|column| row.try_get::<String, _>(column).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect();
        state.insert(table.to_owned(), values);
    }
    state
}

#[tokio::test]
async fn original_personal_server_lease_rechecks_identity_clock_before_renewal() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("personal-server-clock.sqlite");
    let store = Store::sqlite(&database).await.unwrap();
    let encoded = percent_encoding::utf8_percent_encode(
        database.to_str().unwrap(),
        percent_encoding::NON_ALPHANUMERIC,
    )
    .to_string();
    let pool = AnyPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                for pragma in ["PRAGMA foreign_keys=ON", "PRAGMA busy_timeout=10000"] {
                    sqlx::query(pragma).execute(&mut *connection).await?;
                }
                Ok(())
            })
        })
        .connect(&format!("sqlite:{encoded}?mode=rw"))
        .await
        .unwrap();
    let identity = store.identity();
    let bootstrap = "synthetic-personal-server-clock-bootstrap-only-00000000000000";
    identity
        .initialize_credentials(
            bootstrap,
            "synthetic-personal-server-clock-settlement-only-0000000000000",
        )
        .await
        .unwrap();
    let (_, issued) = identity
        .bootstrap(
            bootstrap,
            "personal-server-clock-owner",
            "Personal server clock owner",
            "SyntheticPersonalServerClockPassword-2026!",
            "Personal server clock fixture",
        )
        .await
        .unwrap();
    let actor = identity.authenticate_session(&issued.token).await.unwrap();
    let project = store
        .android_personal_create_project(
            &actor,
            json!({
                "privacy":"private_local", "team_id":null, "name":"Original private server project",
                "description":"", "policy":{},
                "preferred_executor_target":{"kind":"server_linux","pool_id":null}
            }),
        )
        .await
        .unwrap();
    let project_id = id(&project, "id");
    let thread = store
        .android_personal_create_thread(
            &actor,
            json!({
                "project_id":project_id, "title":"Original private server thread",
                "forked_from_thread_id":null, "forked_from_message_id":null
            }),
            None,
        )
        .await
        .unwrap();
    let thread_id = id(&thread, "id");
    let key = KeyId {
        instance: Uuid::new_v4(),
        key: Uuid::new_v4(),
    };
    let profile = store
        .android_personal_provider_mutate(
            &actor,
            "create",
            None,
            json!({
                "team_id":null, "name":"Synthetic no-network provider",
                "provider_kind":"openai_compatible", "model_defaults":{
                    "base_url":"https://fixture.invalid/v1", "model":"fixture",
                    "auth_mode":"none", "context_tokens":65536
                }
            }),
            Some("server-clock-provider"),
            &Keys,
            key,
        )
        .await
        .unwrap();
    let request = json!({
        "project_id":project_id, "thread_id":thread_id,
        "project_revision":project["revision"], "project_privacy":"private_local",
        "executor_target":{"kind":"server_linux","pool_id":null},
        "required_capabilities":[], "model_profile_id":profile["id"],
        "snapshot_id":null, "task":null,
        "input":{"prompt":"Original server lease only; no execution",
                 "provider_binding":android_personal_provider::binding(&profile).unwrap()},
        "idempotency_key":"personal-server-clock-original"
    });
    let original = store
        .android_private_server_create(&actor, &request)
        .await
        .unwrap();
    let run = id(&original["spec"], "id");
    let claimed = store.android_private_server_claim().await.unwrap().unwrap();
    assert_eq!(claimed.run, run);
    assert_eq!(claimed.value["spec"], original["spec"]);
    let lease = claimed.lease;
    // The actual original API operation is successful before the isolated fault.
    store
        .android_private_server_renew(run, lease)
        .await
        .unwrap();
    let binding = sqlx::query("SELECT owner_id,session_id,session_revision,lease_id,lease_until,state FROM android_private_server_runs WHERE id=$1")
        .bind(run.to_string()).fetch_one(&pool).await.unwrap();
    assert_eq!(
        binding.try_get::<String, _>("owner_id").unwrap(),
        actor.identity_id().to_string()
    );
    assert_eq!(
        binding.try_get::<String, _>("session_id").unwrap(),
        actor.session_id().to_string()
    );
    assert_eq!(
        binding.try_get::<String, _>("lease_id").unwrap(),
        lease.to_string()
    );
    assert_eq!(binding.try_get::<String, _>("state").unwrap(), "running");
    let lease_expiry: i64 = binding.try_get("lease_until").unwrap();
    let session_revision: i64 = binding.try_get("session_revision").unwrap();
    let session = sqlx::query(
        "SELECT identity_id,revision,expires_at,revoked FROM identity_sessions WHERE id=$1",
    )
    .bind(actor.session_id().to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        session.try_get::<String, _>("identity_id").unwrap(),
        actor.identity_id().to_string()
    );
    assert_eq!(
        session.try_get::<i64, _>("revision").unwrap(),
        session_revision
    );
    assert_eq!(session.try_get::<i64, _>("revoked").unwrap(), 0);
    let session_expiry: i64 = session.try_get("expires_at").unwrap();
    let scoped: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM identity_session_scopes WHERE session_id=$1")
            .bind(actor.session_id().to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(scoped, 0);
    drop(binding);
    drop(session);
    let before = product_state(&pool).await;
    assert_eq!(before["android_private_server_runs"].len(), 1);
    assert!(before["android_private_loop_steps"].is_empty());
    let guard_before: i64 =
        sqlx::query_scalar("SELECT observed_at FROM android_device_guard WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    let highwater_before: i64 =
        sqlx::query_scalar("SELECT utc_highwater FROM identity_installation WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    let future_highwater = highwater_before.checked_add(3_600_000).unwrap();
    let changed = sqlx::query(
        "UPDATE identity_installation SET utc_highwater=$1 WHERE id=1 AND utc_highwater=$2",
    )
    .bind(future_highwater)
    .bind(highwater_before)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(changed.rows_affected(), 1);
    let database_now: i64 =
        sqlx::query_scalar("SELECT CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(guard_before <= database_now && database_now < future_highwater);
    assert!(database_now < lease_expiry && database_now < session_expiry);

    // Original session, expiry, stored lease and actual DB clock remain untouched.
    let denied = store.android_private_server_renew(run, lease).await;
    assert!(
        matches!(&denied, Err(Error::Storage(error))
        if matches!(error.downcast_ref::<IdentityError>(), Some(IdentityError::ClockRollback))),
        "Original private-server renewal did not preserve the shared Identity clock error: {denied:?}"
    );
    let database_after: i64 =
        sqlx::query_scalar("SELECT CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)")
            .fetch_one(&pool)
            .await
            .unwrap();
    let session_after = sqlx::query(
        "SELECT identity_id,revision,expires_at,revoked FROM identity_sessions WHERE id=$1",
    )
    .bind(actor.session_id().to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        session_after.try_get::<String, _>("identity_id").unwrap(),
        actor.identity_id().to_string()
    );
    assert_eq!(
        session_after.try_get::<i64, _>("revision").unwrap(),
        session_revision
    );
    assert_eq!(session_after.try_get::<i64, _>("revoked").unwrap(), 0);
    assert_eq!(
        session_after.try_get::<i64, _>("expires_at").unwrap(),
        session_expiry
    );
    let lease_after: i64 =
        sqlx::query_scalar("SELECT lease_until FROM android_private_server_runs WHERE id=$1")
            .bind(run.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(lease_after, lease_expiry);
    assert!(database_now <= database_after && database_after < future_highwater);
    assert!(database_after < session_expiry && database_after < lease_expiry);
    assert_eq!(
        product_state(&pool).await,
        before,
        "Clock rejection changed original product rows"
    );
    let guard_after: i64 =
        sqlx::query_scalar("SELECT observed_at FROM android_device_guard WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(guard_after, guard_before);
    let highwater_after: i64 =
        sqlx::query_scalar("SELECT utc_highwater FROM identity_installation WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(highwater_after, future_highwater);
    drop(session_after);
    drop(identity);
    store.close().await;
    // Drain connections returning after the first close pass, as Store.close
    // does for its own pool, before removing the SQLite files on Windows.
    loop {
        pool.close().await;
        if pool.size() == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    drop(store);
    drop(pool);
    directory.close().unwrap();
}
