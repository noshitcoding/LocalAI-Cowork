//! Synthetic real SQLite APIs only. No model, provider, runner, service or guest start.
//! These cases cover ordinary retained personal sessions, not Schedule/Resume admission.
use cowork_domain::Error;
use cowork_identity::{IdentityError, Principal};
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
        Ok(Zeroizing::new([43; 32]))
    }
}
fn id(value: &Value, key: &str) -> Uuid {
    value[key].as_str().unwrap().parse().unwrap()
}
fn assert_clock<T>(result: &Result<T, Error>) {
    match result {
        Err(Error::Storage(error))
            if matches!(
                error.downcast_ref::<IdentityError>(),
                Some(IdentityError::ClockRollback)
            ) => {}
        Err(error) => panic!("Expected original Identity ClockRollback, got {error:?}"),
        Ok(_) => panic!("Expected original Identity ClockRollback; original API returned Ok"),
    }
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
        "android_run_waiting_guards",
        "android_private_runner_authorities",
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
#[derive(Debug, PartialEq)]
struct OriginalSession {
    identity: String,
    revision: i64,
    expires: i64,
    revoked: i64,
    scopes: i64,
}
struct Fixture {
    directory: tempfile::TempDir,
    store: Store,
    pool: AnyPool,
    actor: Principal,
    project: Value,
    profile: Value,
    key: KeyId,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("private-server-central-clock.sqlite");
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
        let bootstrap = "synthetic-server-central-bootstrap-only-000000000000000000";
        store
            .identity()
            .initialize_credentials(
                bootstrap,
                "synthetic-server-central-settlement-only-00000000000000000",
            )
            .await
            .unwrap();
        let (_, issued) = store
            .identity()
            .bootstrap(
                bootstrap,
                "server-central-owner",
                "Server central owner",
                "SyntheticCentralFixturePassword-2026!",
                "Central clock fixture",
            )
            .await
            .unwrap();
        let actor = store
            .identity()
            .authenticate_session(&issued.token)
            .await
            .unwrap();
        let project = store.android_personal_create_project(&actor, json!({
            "privacy":"private_local", "team_id":null, "name":"Original central clock project",
            "description":"", "policy":{},
            "preferred_executor_target":{"kind":"server_linux","pool_id":null}
        })).await.unwrap();
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
                Some("central-clock-provider"),
                &Keys,
                key,
            )
            .await
            .unwrap();
        Self {
            directory,
            store,
            pool,
            actor,
            project,
            profile,
            key,
        }
    }
    async fn queue(&self, key: &str) -> (Uuid, Value) {
        self.queue_with_runner(key, None, None).await
    }
    async fn queue_with_runner(
        &self,
        key: &str,
        runner: Option<&Value>,
        stamp: Option<&Value>,
    ) -> (Uuid, Value) {
        let thread = self
            .store
            .android_personal_create_thread(
                &self.actor,
                json!({
                    "project_id":self.project["id"], "title":key,
                    "forked_from_thread_id":null, "forked_from_message_id":null
                }),
                None,
            )
            .await
            .unwrap();
        let mut request = json!({
            "project_id":self.project["id"], "thread_id":thread["id"],
            "project_revision":self.project["revision"], "project_privacy":"private_local",
            "executor_target":{"kind":"server_linux","pool_id":null},
            "required_capabilities":[], "model_profile_id":self.profile["id"],
            "snapshot_id":null, "task":null,
            "input":{"prompt":"Original API admission only; no execution",
                     "provider_binding":android_personal_provider::binding(&self.profile).unwrap()},
            "idempotency_key":key
        });
        if let Some(runner) = runner {
            request["input"]["frozen_private_runner"] = runner.clone();
            request["input"]["private_runner_authority"] = stamp.unwrap().clone();
        }
        let original = self
            .store
            .android_private_server_create(&self.actor, &request)
            .await
            .unwrap();
        (id(&original["spec"], "id"), original)
    }
    // Existing android_private_loop Store-API fixture pattern. Registration and
    // admission produce a real synthetic-DB stamp, not physical runtime proof.
    async fn observer_queue(&self, key: &str) -> (Uuid, Value) {
        let runner = json!({"format":2,"runtime":"runsc","network":"none",
            "host_id":Uuid::new_v4(),"engine_id":"synthetic-central-observer-engine",
            "image":"fixture@sha256:".to_owned()+&"a".repeat(64),
            "supervisor_uid":0,"process_uid":1000,
            "capabilities":["SETUID","SETGID","KILL","DAC_OVERRIDE","CHOWN"],
            "programs":{},"browser":true,"office":false,"github":false});
        self.store
            .android_private_runner_register(Some(&runner))
            .await
            .unwrap();
        let stamp = self
            .store
            .android_private_runner_admission(&self.actor, id(&self.project, "id"), &runner)
            .await
            .unwrap();
        let queued = self
            .queue_with_runner(key, Some(&runner), Some(&stamp))
            .await;
        assert_eq!(queued.1["spec"]["input"]["frozen_private_runner"], runner);
        queued
    }
    async fn session(&self) -> OriginalSession {
        let row = sqlx::query(
            "SELECT identity_id,revision,expires_at,revoked FROM identity_sessions WHERE id=$1",
        )
        .bind(self.actor.session_id().to_string())
        .fetch_one(&self.pool)
        .await
        .unwrap();
        let scopes =
            sqlx::query_scalar("SELECT COUNT(*) FROM identity_session_scopes WHERE session_id=$1")
                .bind(self.actor.session_id().to_string())
                .fetch_one(&self.pool)
                .await
                .unwrap();
        OriginalSession {
            identity: row.try_get("identity_id").unwrap(),
            revision: row.try_get("revision").unwrap(),
            expires: row.try_get("expires_at").unwrap(),
            revoked: row.try_get("revoked").unwrap(),
            scopes,
        }
    }
    async fn now(&self) -> i64 {
        sqlx::query_scalar("SELECT CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    async fn guard(&self) -> i64 {
        sqlx::query_scalar("SELECT observed_at FROM android_device_guard WHERE id=1")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    async fn floor(&self) -> i64 {
        sqlx::query_scalar("SELECT utc_highwater FROM identity_installation WHERE id=1")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    async fn future_floor(&self) -> i64 {
        let old: i64 =
            sqlx::query_scalar("SELECT utc_highwater FROM identity_installation WHERE id=1")
                .fetch_one(&self.pool)
                .await
                .unwrap();
        let future = old.checked_add(3_600_000).unwrap();
        let changed = sqlx::query(
            "UPDATE identity_installation SET utc_highwater=$1 WHERE id=1 AND utc_highwater=$2",
        )
        .bind(future)
        .bind(old)
        .execute(&self.pool)
        .await
        .unwrap();
        assert_eq!(changed.rows_affected(), 1);
        assert!(self.now().await < future);
        future
    }
    async fn payload(&self, run: Uuid) -> Value {
        let body: String =
            sqlx::query_scalar("SELECT payload FROM android_private_server_runs WHERE id=$1")
                .bind(run.to_string())
                .fetch_one(&self.pool)
                .await
                .unwrap();
        serde_json::from_str(&body).unwrap()
    }
    async fn close(self) {
        self.store.close().await;
        // Drain connections returning after the first close pass, as Store.close
        // does for its own pool, before removing the SQLite files on Windows.
        loop {
            self.pool.close().await;
            if self.pool.size() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        drop(self.store);
        drop(self.pool);
        self.directory.close().unwrap();
    }
}

#[tokio::test]
async fn queued_original_clock_failure_rolls_back_without_authority_changed() {
    let f = Fixture::new().await;
    let (control, _) = f.queue("positive-claim-control").await;
    let admitted = f
        .store
        .android_private_server_claim()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(admitted.run, control);
    f.store
        .android_private_server_finish(
            control,
            admitted.lease,
            None,
            Some(&json!({"code":"synthetic_control_finished"})),
        )
        .await
        .unwrap();
    let (run, original) = f.queue("clock-queued-original").await;
    let session = f.session().await;
    assert_eq!(session.identity, f.actor.identity_id().to_string());
    assert_eq!((session.revoked, session.scopes), (0, 0));
    let before = product_state(&f.pool).await;
    let guard = f.guard().await;
    let floor = f.future_floor().await;
    let now = f.now().await;
    assert!(guard <= now && now < floor && now < session.expires);
    let result = f.store.android_private_server_claim().await;
    assert_clock(&result);
    assert_eq!(
        product_state(&f.pool).await,
        before,
        "Clock failure changed the original queue/product rows"
    );
    assert_eq!(f.guard().await, guard);
    assert_eq!(f.floor().await, floor);
    assert_eq!(f.session().await, session);
    let after = f.now().await;
    assert!(now <= after && after < floor && after < session.expires);
    let current = f.payload(run).await;
    assert_eq!(current, original);
    assert_eq!(current["state"], "queued");
    let lease: Option<String> =
        sqlx::query_scalar("SELECT lease_id FROM android_private_server_runs WHERE id=$1")
            .bind(run.to_string())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(lease.is_none());
    f.close().await;
}

#[tokio::test]
async fn original_received_survives_clock_loss_while_new_effect_admission_is_denied() {
    let f = Fixture::new().await;
    let (first, _) = f.queue("received-original").await;
    let first_lease = f
        .store
        .android_private_server_claim()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_lease.run, first);
    let first_profile = f
        .store
        .android_private_server_profile(first, first_lease.lease)
        .await
        .unwrap();
    let intent = json!({"provider_binding":android_personal_provider::binding(&first_profile).unwrap(),
        "provider_sha256":cowork_domain::canonical_hash(&first_profile),
        "request_sha256":"synthetic-original-request","format":"synthetic-store-admission-only"});
    assert!(
        f.store
            .android_private_server_dispatch(first, first_lease.lease, &intent, &Keys)
            .await
            .unwrap()
            .is_none()
    );
    let (second, _) = f.queue("new-effect-after-clock").await;
    let second_lease = f
        .store
        .android_private_server_claim()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_lease.run, second);
    let second_profile = f
        .store
        .android_private_server_profile(second, second_lease.lease)
        .await
        .unwrap();
    assert_eq!(second_profile, first_profile);
    assert_eq!(
        intent["provider_sha256"],
        json!(cowork_domain::canonical_hash(&second_profile))
    );
    let first_expiry: i64 =
        sqlx::query_scalar("SELECT lease_until FROM android_private_server_runs WHERE id=$1")
            .bind(first.to_string())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let second_expiry: i64 =
        sqlx::query_scalar("SELECT lease_until FROM android_private_server_runs WHERE id=$1")
            .bind(second.to_string())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let before = product_state(&f.pool).await;
    let session = f.session().await;
    let guard = f.guard().await;
    let floor = f.future_floor().await;
    let now = f.now().await;
    assert!(
        guard <= now
            && now < floor
            && now < session.expires
            && now < first_expiry
            && now < second_expiry
    );
    let denied = f
        .store
        .android_private_server_dispatch(second, second_lease.lease, &intent, &Keys)
        .await;
    assert_clock(&denied);
    assert_eq!(product_state(&f.pool).await, before);
    assert_eq!(f.guard().await, guard);
    assert_eq!(f.floor().await, floor);
    assert_eq!(f.session().await, session);
    let after = f.now().await;
    assert!(
        now <= after
            && after < floor
            && after < session.expires
            && after < first_expiry
            && after < second_expiry
    );
    // These are bounded synthetic bytes delivered to the existing retention API,
    // not a claim of a real provider request, completed network stream or guest.
    let received = json!({"content":"bounded original response", "usage":{},
        "uncertain":false, "wire_evidence":{"completed_stream":true}});
    f.store
        .android_private_server_received(first, first_lease.lease, &received)
        .await
        .unwrap();
    let retained = product_state(&f.pool).await;
    f.store
        .android_private_server_received(first, first_lease.lease, &received)
        .await
        .unwrap();
    assert_eq!(product_state(&f.pool).await, retained);
    let mut changed = received.clone();
    changed["content"] = json!("different bytes");
    assert!(matches!(
        f.store
            .android_private_server_received(first, first_lease.lease, &changed)
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(product_state(&f.pool).await, retained);
    let final_value = f
        .store
        .android_private_server_finish(
            first,
            first_lease.lease,
            Some(&json!({"content":received["content"],"usage":received["usage"]})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(final_value["state"], "interrupted");
    assert!(final_value["result"].is_null());
    let stored: String =
        sqlx::query_scalar("SELECT effect_received FROM android_private_server_runs WHERE id=$1")
            .bind(first.to_string())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), received);
    let second_intent: Option<String> =
        sqlx::query_scalar("SELECT effect_intent FROM android_private_server_runs WHERE id=$1")
            .bind(second.to_string())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(second_intent.is_none());
    let empty_steps: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM android_private_loop_steps")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(empty_steps, 0);
    assert_eq!(f.floor().await, floor);
    assert_eq!(f.session().await, session);
    f.close().await;
}

#[tokio::test]
async fn actual_original_session_revocation_still_invalidates_the_queue() {
    let f = Fixture::new().await;
    let (run, _) = f.queue("revoked-original-session").await;
    let session = f.session().await;
    f.store
        .identity()
        .revoke_account_session(&f.actor, f.actor.session_id(), session.revision)
        .await
        .unwrap();
    assert!(
        f.store
            .android_private_server_claim()
            .await
            .unwrap()
            .is_none()
    );
    let result = f.payload(run).await;
    assert_eq!(result["state"], "failed");
    assert_eq!(result["error"]["code"], "authority_changed");
    f.close().await;
}

#[tokio::test]
async fn actual_frozen_provider_deletion_still_invalidates_the_queue() {
    let f = Fixture::new().await;
    let (run, _) = f.queue("deleted-original-provider").await;
    f.store
        .android_personal_provider_mutate(
            &f.actor,
            "delete",
            Some(id(&f.profile, "id")),
            json!({"expected_revision":f.profile["revision"]}),
            Some("delete-original-provider"),
            &Keys,
            f.key,
        )
        .await
        .unwrap();
    assert!(
        f.store
            .android_private_server_claim()
            .await
            .unwrap()
            .is_none()
    );
    let result = f.payload(run).await;
    assert_eq!(result["state"], "failed");
    assert_eq!(result["error"]["code"], "authority_changed");
    f.close().await;
}

#[tokio::test]
async fn original_runner_capacity_observer_preserves_clock_error_and_queued_rows() {
    let f = Fixture::new().await;
    let (run, original) = f.observer_queue("original-runner-capacity").await;
    // No runner was supplied, so this is the actual mismatched original-runner
    // observer arm, not the compatible-runner or running-lease path.
    let control = f
        .store
        .android_private_server_claim_observed(None, Some("synthetic-central-observer"))
        .await
        .unwrap();
    assert!(control.is_none());
    let waiting =
        sqlx::query("SELECT reason,host_slot FROM android_run_waiting_guards WHERE run_id=$1")
            .bind(run.to_string())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        waiting.try_get::<String, _>("reason").unwrap(),
        "runner_capacity"
    );
    assert_eq!(
        waiting.try_get::<String, _>("host_slot").unwrap(),
        "synthetic-central-observer"
    );
    drop(waiting);
    assert_eq!(f.payload(run).await, original);
    let before = product_state(&f.pool).await;
    let session = f.session().await;
    let guard = f.guard().await;
    let floor = f.future_floor().await;
    let now = f.now().await;
    assert!(guard <= now && now < floor && now < session.expires);
    let result = f
        .store
        .android_private_server_claim_observed(None, Some("synthetic-central-observer"))
        .await;
    assert_clock(&result);
    assert_eq!(
        product_state(&f.pool).await,
        before,
        "Observer swallowed the clock error or changed original annotations/receipts"
    );
    assert_eq!(f.guard().await, guard);
    assert_eq!(f.floor().await, floor);
    assert_eq!(f.session().await, session);
    assert_eq!(f.payload(run).await, original);
    let no_lease: Option<String> =
        sqlx::query_scalar("SELECT lease_id FROM android_private_server_runs WHERE id=$1")
            .bind(run.to_string())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(no_lease.is_none());
    let after = f.now().await;
    assert!(now <= after && after < floor && after < session.expires);
    f.close().await;
}

#[tokio::test]
async fn original_queued_sql_failure_rolls_back_in_claim_and_observer() {
    for observer in [false, true] {
        let f = Fixture::new().await;
        let (run, original) = if observer {
            let queued = f
                .observer_queue("original-observer-technical-failure")
                .await;
            assert!(
                f.store
                    .android_private_server_claim_observed(
                        None,
                        Some("synthetic-technical-observer")
                    )
                    .await
                    .unwrap()
                    .is_none()
            );
            queued
        } else {
            f.queue("original-technical-failure").await
        };
        let before = product_state(&f.pool).await;
        let session = f.session().await;
        let guard = f.guard().await;
        let original_floor = f.floor().await;
        let now = f.now().await;
        assert!(guard <= now && now < session.expires);
        // Isolated own DB fault only: temporarily make the original authority
        // SELECT's actual table unavailable. No run/grant/lease data are forged.
        sqlx::query(
            "ALTER TABLE identity_sessions RENAME TO synthetic_unavailable_identity_sessions",
        )
        .execute(&f.pool)
        .await
        .unwrap();
        let denied = if observer {
            f.store
                .android_private_server_claim_observed(None, Some("synthetic-technical-observer"))
                .await
        } else {
            f.store.android_private_server_claim().await
        };
        // Restore before assertion/preservation reads even after unexpected Ok;
        // the exact original API result is retained for the assertion.
        sqlx::query(
            "ALTER TABLE synthetic_unavailable_identity_sessions RENAME TO identity_sessions",
        )
        .execute(&f.pool)
        .await
        .unwrap();
        assert!(
            matches!(&denied, Err(Error::Storage(error))
            if matches!(error.downcast_ref::<sqlx::Error>(), Some(sqlx::Error::Database(database))
                if database.message()=="no such table: identity_sessions")),
            "Original claim/observer did not preserve the actual SQLite authority-query failure; observer={observer}"
        );
        assert_eq!(
            product_state(&f.pool).await,
            before,
            "Technical failure changed original rows; observer={observer}"
        );
        assert_eq!(f.guard().await, guard);
        assert_eq!(f.floor().await, original_floor);
        assert_eq!(f.session().await, session);
        assert_eq!(f.payload(run).await, original);
        let after = f.now().await;
        assert!(now <= after && after < session.expires);
        f.close().await;
    }
}
