//! Synthetic personal-device Identity-clock regression; no executor or network.
use cowork_contracts::{DeviceCapability, DeviceExecutorRegistration, PairDeviceExecutor};
use cowork_storage::{AndroidDeviceRunRequest, Store};
use serde_json::{Value, json};
use sqlx::{AnyPool, Row, any::AnyPoolOptions};
use std::collections::BTreeMap;
use uuid::Uuid;

const AGENT_TOKEN: &str = "synthetic-personal-device-clock-agent-not-a-real-secret-000000000";

fn id(value: &Value, key: &str) -> Uuid {
    value[key].as_str().unwrap().parse().unwrap()
}

// Every original column is compared, including NULL/blob values. Identity and
// clock rows are checked separately; this is logical-value, not DB-file equality.
type ProductState = BTreeMap<String, Vec<Vec<String>>>;

async fn product_state(pool: &AnyPool) -> ProductState {
    let mut state = BTreeMap::new();
    for table in [
        "android_scoped_runs",
        "android_scoped_events",
        "android_scoped_snapshots",
        "android_scoped_snapshot_chunks",
        "android_scoped_desktop_sessions",
        "android_device_tickets",
        "android_hub_connections",
        "android_hub_streams",
        "android_hub_forwards",
        "android_scoped_artifacts",
        "android_scoped_checkpoints",
        "android_scoped_requests",
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
            .collect::<Vec<_>>();
        state.insert(table.to_owned(), values);
    }
    state
}

#[tokio::test]
async fn original_personal_lease_rechecks_identity_clock_before_renewal() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("personal-device-clock.sqlite");
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
    let bootstrap = "synthetic-personal-device-clock-bootstrap-only-000000000000000";
    identity
        .initialize_credentials(
            bootstrap,
            "synthetic-personal-device-clock-settlement-only-000000000000000",
        )
        .await
        .unwrap();
    let (_, issued) = identity
        .bootstrap(
            bootstrap,
            "personal-device-clock-owner",
            "Personal device clock owner",
            "SyntheticPersonalDeviceClockPassword-2026!",
            "Personal device clock fixture",
        )
        .await
        .unwrap();
    let owner = identity.authenticate_session(&issued.token).await.unwrap();
    let project = store
        .android_personal_create_project(
            &owner,
            json!({"privacy":"private_local","name":"Original personal project","description":"","policy":{}}),
        )
        .await
        .unwrap();
    let project_id = id(&project, "id");
    let thread = store
        .android_personal_create_thread(
            &owner,
            json!({"project_id":project_id,"title":"Original personal thread"}),
            None,
        )
        .await
        .unwrap();
    let device = Uuid::new_v4();
    store
        .android_pair_device(
            &owner,
            PairDeviceExecutor {
                team_id: None,
                registration: DeviceExecutorRegistration {
                    schema_version: 2,
                    protocol_version: 2,
                    executor_id: device,
                    kind: "personal_device".into(),
                    pool_id: None,
                    owner_user_id: None,
                    display_name: "Synthetic personal device".into(),
                    max_concurrent_runs: 1,
                    capabilities: vec![DeviceCapability {
                        schema_version: 2,
                        name: "desktop.windows".into(),
                        version: "synthetic-no-device-start".into(),
                        attributes: Default::default(),
                    }],
                    labels: Default::default(),
                    personal_device_remote_control: Some("unattended".into()),
                },
            },
        )
        .await
        .unwrap();
    store
        .android_create_device_credential(
            &owner,
            device,
            "Synthetic clock fixture",
            None,
            AGENT_TOKEN,
        )
        .await
        .unwrap();
    let agent = store
        .android_agent_authenticate(device, AGENT_TOKEN)
        .await
        .unwrap();
    let original = store
        .android_device_create_run(
            &owner,
            AndroidDeviceRunRequest {
                team: None,
                project: project_id,
                order: id(&thread, "id"),
                project_revision: project["revision"].as_i64().unwrap(),
                target: json!({"kind":"personal_device","device_id":device}),
                capabilities: vec!["desktop.windows".into()],
                input: json!({"prompt":"Original personal fixture input"}),
                model_profile: None,
                snapshot: None,
                key: "personal-clock-original".into(),
                task: None,
            },
        )
        .await
        .unwrap();
    let run = id(&original["spec"], "id");
    let claimed = store.android_device_claim(&agent).await.unwrap().unwrap();
    assert_eq!(claimed["run"]["spec"], original["spec"]);
    let lease = id(&claimed, "lease_token");

    // Same genuine original operation succeeds before the isolated clock change.
    let positive = store
        .android_device_lease_heartbeat(&agent, run, lease)
        .await
        .unwrap();
    assert_eq!(id(&positive, "lease_token"), lease);
    assert_eq!(positive["run"]["spec"], original["spec"]);
    let binding = sqlx::query(
        "SELECT creator_id,creator_session_id,lease_expires_at FROM android_scoped_runs WHERE id=$1",
    )
    .bind(run.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        binding.try_get::<String, _>("creator_id").unwrap(),
        owner.identity_id().to_string()
    );
    assert_eq!(
        binding.try_get::<String, _>("creator_session_id").unwrap(),
        owner.session_id().to_string()
    );
    let lease_expiry: i64 = binding.try_get("lease_expires_at").unwrap();
    drop(binding);
    let before = product_state(&pool).await;
    assert_eq!(before["android_scoped_runs"].len(), 1);
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
    let session_expiry: i64 =
        sqlx::query_scalar("SELECT expires_at FROM identity_sessions WHERE id=$1")
            .bind(owner.session_id().to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(guard_before <= database_now && database_now < future_highwater);
    assert!(database_now < lease_expiry && database_now < session_expiry);

    // No session/lease/credential expiry, original token or actual clock is changed.
    // Revalidate the same original operation before renewing its lease.
    let denied = store
        .android_device_lease_heartbeat(&agent, run, lease)
        .await;
    assert!(
        denied.is_err(),
        "Original personal lease ignored the shared Identity clock fence"
    );
    // Preserve the original bounds and check after the awaited denial as well:
    // a delayed expiry must not masquerade as an Identity-clock rejection.
    let database_after: i64 =
        sqlx::query_scalar("SELECT CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)")
            .fetch_one(&pool)
            .await
            .unwrap();
    let session_expiry_after: i64 =
        sqlx::query_scalar("SELECT expires_at FROM identity_sessions WHERE id=$1")
            .bind(owner.session_id().to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    let lease_expiry_after: i64 =
        sqlx::query_scalar("SELECT lease_expires_at FROM android_scoped_runs WHERE id=$1")
            .bind(run.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(session_expiry_after, session_expiry);
    assert_eq!(lease_expiry_after, lease_expiry);
    assert!(database_now <= database_after && database_after < future_highwater);
    assert!(database_after < session_expiry && database_after < lease_expiry);
    assert_eq!(
        product_state(&pool).await,
        before,
        "Clock denial changed the original run, lease or product rows"
    );
    let guard_after: i64 =
        sqlx::query_scalar("SELECT observed_at FROM android_device_guard WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(guard_after, guard_before);
    let retained_highwater: i64 =
        sqlx::query_scalar("SELECT utc_highwater FROM identity_installation WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(retained_highwater, future_highwater);
    drop(identity);
    store.close().await;
    pool.close().await;
    drop(store);
    drop(pool);
    directory.close().unwrap();
}
