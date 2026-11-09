//! Opt-in disposable PostgreSQL Store/Identity tests; no HTTP, device, model or tool start.
//! Each exact test requires its OWN NEW EMPTY cowork_device_actor_* database.
use cowork_contracts::{
    CreateTeamOrder, CreateTeamProject, DeviceCapability, DeviceExecutorRegistration,
    PairDeviceExecutor, TeamRunPlan,
};
use cowork_domain::{Error, Result};
use cowork_identity::Principal;
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{
    AndroidAgentPrincipal, AndroidDesktopAccess, AndroidDeviceRunRequest, AndroidHubOwner, Store,
    team_service::{TeamExecution, TeamExecutionResult, TeamExecutor, TeamService},
};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Any, AnyPool, any::AnyPoolOptions, pool::PoolConnection};
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;
use zeroize::Zeroizing;

const INPUT: &[u8] = b"Original input only\nGr\xc3\xbc\xc3\x9fe 119,00\n";
const INPUT_PATH: &str = "original.txt";
const AGENT_TOKEN: &str = "synthetic-device-actor-credential-not-a-real-secret-0000000000";

struct NoKeys;
impl KeyStore for NoKeys {
    fn load(&self, _: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        panic!("Device authority fixture must never load a private key")
    }
}
struct NoExecution;
impl TeamExecutor for NoExecution {
    fn supports(&self, _: &TeamRunPlan) -> bool {
        false
    }
    fn execute(&self, _: TeamExecution) -> BoxFuture<'_, Result<TeamExecutionResult>> {
        panic!("Device authority fixture must never execute a model or tool")
    }
}

fn id(value: &Value, key: &str) -> Uuid {
    value[key].as_str().unwrap().parse().unwrap()
}
fn run_id(value: &Value) -> Uuid {
    id(&value["spec"], "id")
}
fn digest() -> String {
    format!("{:x}", Sha256::digest(INPUT))
}
fn token(purpose: &str) -> String {
    format!(
        "synthetic-{purpose}-{}-not-a-real-credential",
        Uuid::new_v4()
    )
}
fn forbidden<T>(result: Result<T>) {
    match result {
        Err(Error::Forbidden) => (),
        Err(_) => panic!("Expected Forbidden, received a different error"),
        Ok(_) => panic!("Operation succeeded after its actor authority changed"),
    }
}

// All columns remain present, including NULLs and opaque payload strings.
// Only row order is normalized; array/payload contents are never sorted away.
type ProductState = BTreeMap<String, Vec<Value>>;
const PG_ENV: &str = "COWORK_DEVICE_ACTOR_PG_DATABASE_URL";
const PG_PREFIX: &str = "cowork_device_actor_";
const PG_LOCK: i64 = 8_732_460_315;

struct Fixture {
    store: Store,
    pool: AnyPool,
    guard: PoolConnection<Any>,
    service: TeamService,
    owner: Principal,
    team: Uuid,
    project: Uuid,
    order: Uuid,
    device: Uuid,
    agent: AndroidAgentPrincipal,
}
impl Fixture {
    async fn new() -> Self {
        // No fallback, database creation/drop, admin connection or URL logging.
        let url = std::env::var(PG_ENV)
            .unwrap_or_else(|_| panic!("Set the explicit disposable PostgreSQL test URL"));
        let parsed = reqwest::Url::parse(&url)
            .unwrap_or_else(|_| panic!("Invalid disposable PostgreSQL URL"));
        assert!(matches!(parsed.scheme(), "postgres" | "postgresql"));
        assert!(parsed.host_str().is_some() && parsed.fragment().is_none());
        let query = parsed.query_pairs().collect::<Vec<_>>();
        assert!(
            query.len() <= 1
                && query.iter().all(|(key, value)| {
                    key == "sslmode"
                        && matches!(
                            value.as_ref(),
                            "disable"
                                | "allow"
                                | "prefer"
                                | "require"
                                | "verify-ca"
                                | "verify-full"
                        )
                }),
            "Only the optional sslmode URL query is accepted"
        );
        let requested = percent_encoding::percent_decode_str(parsed.path().trim_start_matches('/'))
            .decode_utf8()
            .unwrap_or_else(|_| panic!("Invalid database name encoding"));
        assert!(requested.len() <= 63 && requested.starts_with(PG_PREFIX));
        let suffix = requested.strip_prefix(PG_PREFIX).unwrap();
        assert!(
            suffix.len() >= 8
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
            "Use a new disposable database with the fixed test prefix"
        );
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .unwrap_or_else(|_| panic!("Cannot connect to the disposable PostgreSQL database"));
        let mut guard = pool.acquire().await.unwrap();
        let database: String = sqlx::query_scalar("SELECT current_database()::text")
            .fetch_one(&mut *guard)
            .await
            .unwrap();
        assert_eq!(
            database,
            requested.as_ref(),
            "Connected database differs from the explicit test URL"
        );
        let locked: i64 = sqlx::query_scalar(
            "SELECT CASE WHEN pg_try_advisory_lock($1) THEN 1 ELSE 0 END::bigint",
        )
        .bind(PG_LOCK)
        .fetch_one(&mut *guard)
        .await
        .unwrap();
        assert_eq!(
            locked, 1,
            "Another test already owns this disposable database"
        );
        let schema: String = sqlx::query_scalar("SELECT COALESCE(current_schema(),'')::text")
            .fetch_one(&mut *guard)
            .await
            .unwrap();
        assert_eq!(
            schema, "public",
            "Unexpected initial PostgreSQL search path"
        );
        // Check before the FIRST Store::connect/migration/bootstrap/mutation.
        // The held original session lock prevents two cooperating tests from
        // passing this emptiness gate concurrently. It grants no product rights.
        let objects: i64 = sqlx::query_scalar(
            r#"
            SELECT (
                (SELECT COUNT(*) FROM pg_catalog.pg_class c
                    JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
                    WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema')
                + (SELECT COUNT(*) FROM pg_catalog.pg_proc p
                    JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace
                    WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema')
                + (SELECT COUNT(*) FROM pg_catalog.pg_type t
                    JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace
                    WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema')
                + (SELECT COUNT(*) FROM pg_catalog.pg_namespace n
                    WHERE n.nspname !~ '^pg_' AND n.nspname NOT IN ('information_schema','public'))
                + (SELECT COUNT(*) FROM pg_catalog.pg_extension WHERE extname <> 'plpgsql')
            )::bigint
        "#,
        )
        .fetch_one(&mut *guard)
        .await
        .unwrap();
        assert_eq!(
            objects, 0,
            "Refusing to initialize a nonempty disposable database"
        );
        let store = Store::connect(&url)
            .await
            .unwrap_or_else(|_| panic!("Cannot initialize the checked-empty disposable database"));
        let identity = store.identity();
        let bootstrap = "synthetic-device-actor-bootstrap-only-000000000000000000";
        identity
            .initialize_credentials(
                bootstrap,
                "synthetic-device-actor-settlement-only-000000000000000000",
            )
            .await
            .unwrap();
        let (organization, issued) = identity
            .bootstrap(
                bootstrap,
                "pg-device-owner",
                "PG device owner",
                "SyntheticDeviceActorPassword-2026!",
                "PG device team",
            )
            .await
            .unwrap();
        let owner = identity.authenticate_session(&issued.token).await.unwrap();
        let team = organization.id;
        let service = TeamService::new(
            store.clone(),
            Arc::new(NoKeys),
            KeyId {
                instance: Uuid::new_v4(),
                key: Uuid::new_v4(),
            },
            Arc::new(NoExecution),
        );
        let project = service
            .create_project(
                &owner,
                team,
                CreateTeamProject {
                    name: "PG device authority project".into(),
                    idempotency_key: "pg-device-project".into(),
                },
            )
            .await
            .unwrap()
            .id;
        let order = service
            .create_order(
                &owner,
                team,
                project,
                CreateTeamOrder {
                    title: "PG device authority order".into(),
                    prompt: "Synthetic input only".into(),
                    idempotency_key: "pg-device-order".into(),
                },
            )
            .await
            .unwrap()
            .id;
        let device = Uuid::new_v4();
        store
            .android_pair_device(
                &owner,
                PairDeviceExecutor {
                    team_id: Some(team),
                    registration: DeviceExecutorRegistration {
                        schema_version: 2,
                        protocol_version: 2,
                        executor_id: device,
                        kind: "personal_device".into(),
                        pool_id: None,
                        owner_user_id: None,
                        display_name: "PG synthetic device".into(),
                        max_concurrent_runs: 1,
                        labels: Default::default(),
                        personal_device_remote_control: Some("unattended".into()),
                        capabilities: vec![DeviceCapability {
                            schema_version: 2,
                            name: "desktop.windows".into(),
                            version: "synthetic-no-device-start".into(),
                            attributes: Default::default(),
                        }],
                    },
                },
            )
            .await
            .unwrap();
        store
            .android_create_device_credential(&owner, device, "PG fixture", None, AGENT_TOKEN)
            .await
            .unwrap();
        let agent = store
            .android_agent_authenticate(device, AGENT_TOKEN)
            .await
            .unwrap();
        Self {
            store,
            pool,
            guard,
            service,
            owner,
            team,
            project,
            order,
            device,
            agent,
        }
    }

    async fn scoped(&self) -> Principal {
        let identity = self.store.identity();
        let mut tx = self.pool.begin().await.unwrap();
        let issued = identity
            .issue_verified_scoped_session_with_permissions_in_transaction(
                &mut tx,
                self.owner.identity_id(),
                self.team,
                3_600_000,
                15,
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let actor = identity.authenticate_session(&issued.token).await.unwrap();
        assert_ne!(actor.session_id(), self.owner.session_id());
        actor
    }

    async fn permissions(&self, actor: &Principal, permissions: i64) {
        assert!(matches!(permissions, 1 | 15));
        // Controlled authoritative change in this fixture DB only; no new
        // Principal, session, revision, lease or product record is fabricated.
        let changed =
            sqlx::query("UPDATE identity_session_scopes SET permissions=$1 WHERE session_id=$2")
                .bind(permissions)
                .bind(actor.session_id().to_string())
                .execute(&self.pool)
                .await
                .unwrap();
        assert_eq!(changed.rows_affected(), 1);
    }

    async fn create_run(&self, creator: &Principal) -> Value {
        self.store
            .android_device_create_run_with_files(
                creator,
                AndroidDeviceRunRequest {
                    team: Some(self.team),
                    project: self.project,
                    order: self.order,
                    project_revision: 1,
                    target: json!({"kind":"personal_device","device_id":self.device}),
                    capabilities: vec!["desktop.windows".into()],
                    input: json!({"prompt":"Read the original fixture bytes"}),
                    model_profile: None,
                    snapshot: None,
                    key: format!("device-run-{}", Uuid::new_v4()),
                    task: None,
                },
                vec![(INPUT_PATH.into(), INPUT.to_vec())],
            )
            .await
            .unwrap()
    }

    async fn claim(&self, original: &Value) -> Uuid {
        let lease = self
            .store
            .android_device_claim(&self.agent)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lease["run"]["spec"], original["spec"]);
        assert_eq!(lease["run"]["state"], "running");
        id(&lease, "lease_token")
    }

    async fn assert_original_bytes(&self, run: Uuid, lease: Uuid) {
        let manifest = self
            .store
            .android_agent_get_snapshot(&self.agent, run, lease, true)
            .await
            .unwrap()
            .unwrap();
        let raw: String = sqlx::query_scalar(
            "SELECT payload FROM android_scoped_snapshots WHERE run_id=$1 AND committed=1",
        )
        .bind(run.to_string())
        .fetch_one(&self.pool)
        .await
        .unwrap();
        assert_eq!(manifest, serde_json::from_str::<Value>(&raw).unwrap());
        assert_eq!(manifest["files"].as_array().unwrap().len(), 1);
        assert_eq!(manifest["files"][0]["path"], INPUT_PATH);
        assert_eq!(manifest["files"][0]["digest"], digest());
        assert_eq!(
            manifest["files"][0]["size"].as_u64(),
            Some(INPUT.len() as u64)
        );
        let bytes = self
            .store
            .android_agent_get_chunk(&self.agent, run, lease, &digest())
            .await
            .unwrap();
        assert_eq!(bytes.as_slice(), INPUT);
    }

    async fn product_state(&self) -> ProductState {
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
            // table is exclusively one of the twelve literal identifiers above.
            let raw: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT row_to_json(original_row)::text FROM public.\"{table}\" AS original_row",
            ))
            .fetch_all(&self.pool)
            .await
            .unwrap();
            let mut values = raw
                .into_iter()
                .map(|row| serde_json::from_str::<Value>(&row).unwrap())
                .collect::<Vec<_>>();
            values.sort_by_key(|row| serde_json::to_string(row).unwrap());
            state.insert(table.to_owned(), values);
        }
        state
    }

    async fn denied_lease_operations(&self, run: Uuid, lease: Uuid) {
        forbidden(
            self.store
                .android_device_lease_heartbeat(&self.agent, run, lease)
                .await,
        );
        forbidden(
            self.store
                .android_device_append_event(
                    &self.agent,
                    run,
                    lease,
                    Some(Uuid::new_v4()),
                    "fixture_denied",
                    json!({"unexpected":true}),
                )
                .await,
        );
        forbidden(
            self.store
                .android_agent_get_snapshot(&self.agent, run, lease, true)
                .await,
        );
        forbidden(
            self.store
                .android_agent_get_chunk(&self.agent, run, lease, &digest())
                .await,
        );
    }

    async fn denied_access(
        &self,
        access: &AndroidDesktopAccess,
        hub: &AndroidHubOwner,
        pending_forward: &str,
    ) {
        forbidden(self.store.android_desktop_access_current(access).await);
        forbidden(
            self.store
                .android_desktop_agent_current(&self.agent, access)
                .await,
        );
        forbidden(self.store.android_hub_owner(access).await);
        forbidden(
            self.store
                .android_hub_forward(
                    hub,
                    access,
                    None,
                    None,
                    &token("new-forward-must-not-exist"),
                )
                .await,
        );
        forbidden(
            self.store
                .android_hub_take_forward(hub.node, hub.boot, pending_forward, "desktop")
                .await,
        );
    }

    async fn close(self) {
        let Self {
            store,
            pool,
            mut guard,
            service,
            ..
        } = self;
        drop(service);
        store.close().await;
        drop(store);
        let released: i64 =
            sqlx::query_scalar("SELECT CASE WHEN pg_advisory_unlock($1) THEN 1 ELSE 0 END::bigint")
                .bind(PG_LOCK)
                .fetch_one(&mut *guard)
                .await
                .unwrap();
        assert_eq!(released, 1);
        drop(guard);
        pool.close().await;
        // Root's later runner alone owns retention/drop of this ORIGINAL DB.
    }
}

fn original_run(state: &ProductState, run: Uuid) -> &Value {
    let run = run.to_string();
    state["android_scoped_runs"]
        .iter()
        .find(|row| row["id"].as_str() == Some(run.as_str()))
        .unwrap()
}

fn assert_only_original_queue_interruption(
    original: &Value,
    run: Uuid,
    mut expected: ProductState,
    actual: ProductState,
) {
    let old = original_run(&expected, run);
    let new = original_run(&actual, run);
    assert_eq!(new["state"], "interrupted");
    let revision = old["revision"].as_i64().unwrap() + 1;
    assert_eq!(new["revision"].as_i64(), Some(revision));
    let updated_at = new["updated_at"].as_i64().unwrap();
    assert!(updated_at >= old["updated_at"].as_i64().unwrap());
    let value: Value = serde_json::from_str(new["payload"].as_str().unwrap()).unwrap();
    let payload_time = chrono::DateTime::parse_from_rfc3339(value["updated_at"].as_str().unwrap())
        .unwrap()
        .timestamp_millis();
    assert_eq!(payload_time, updated_at);
    let mut payload = original.clone();
    payload["state"] = json!("interrupted");
    payload["revision"] = json!(revision);
    payload["etag"] = json!(format!("{run}:{revision}"));
    payload["updated_at"] = value["updated_at"].clone();
    payload["error"] = json!({"code":"device_authority_changed","message":"Execution authority changed","retryable":false});
    assert_eq!(value, payload);
    let target = expected
        .get_mut("android_scoped_runs")
        .unwrap()
        .iter_mut()
        .find(|row| row["id"].as_str() == Some(run.to_string().as_str()))
        .unwrap();
    // Exactly save_run's four original columns, never an erased Run table.
    // Lease, original bindings, every other run and all eleven other tables
    // remain exact. NULLs and all opaque payload text are retained.
    for column in ["payload", "state", "revision", "updated_at"] {
        target[column] = new[column].clone();
    }
    for rows in expected.values_mut() {
        rows.sort_by_key(|row| serde_json::to_string(row).unwrap());
    }
    assert_eq!(
        actual, expected,
        "Queue rejection changed original product data"
    );
}

#[tokio::test]
#[ignore = "Requires its OWN NEW EMPTY cowork_device_actor_* database in COWORK_DEVICE_ACTOR_PG_DATABASE_URL"]
async fn postgres_original_creator_scope_fences_queue_and_existing_lease_without_other_changes() {
    let f = Fixture::new().await;
    let queued_creator = f.scoped().await;
    let queued = f.create_run(&queued_creator).await;
    let before = f.product_state().await;
    f.permissions(&queued_creator, 1).await;
    assert!(
        f.store
            .android_device_claim(&f.agent)
            .await
            .unwrap()
            .is_none()
    );
    assert_only_original_queue_interruption(
        &queued,
        run_id(&queued),
        before,
        f.product_state().await,
    );

    // An independently issued genuine scoped session now creates and claims its
    // own new run. The earlier interrupted run remains in every state snapshot.
    let leased_creator = f.scoped().await;
    let original = f.create_run(&leased_creator).await;
    let run = run_id(&original);
    let lease = f.claim(&original).await;
    f.store
        .android_device_lease_heartbeat(&f.agent, run, lease)
        .await
        .unwrap();
    f.assert_original_bytes(run, lease).await;
    f.store
        .android_device_append_event(
            &f.agent,
            run,
            lease,
            Some(Uuid::new_v4()),
            "fixture_original",
            json!({"original":true}),
        )
        .await
        .unwrap();
    f.permissions(&leased_creator, 1).await;
    let before = f.product_state().await;
    f.denied_lease_operations(run, lease).await;
    assert_eq!(f.product_state().await, before);
    f.close().await;
}

#[tokio::test]
#[ignore = "Requires its OWN NEW EMPTY cowork_device_actor_* database in COWORK_DEVICE_ACTOR_PG_DATABASE_URL"]
async fn postgres_separate_desktop_viewer_and_controller_keep_creator_and_original_rows() {
    let f = Fixture::new().await;
    let other_org = f
        .store
        .identity()
        .create_organization(
            &f.owner,
            "Actual other PG organization",
            "pg-other-organization",
        )
        .await
        .unwrap();
    let viewer = f.scoped().await;
    let controller = f.scoped().await;
    assert_ne!(viewer.session_id(), controller.session_id());
    let original = f.create_run(&f.owner).await;
    let run = run_id(&original);
    let lease = f.claim(&original).await;
    let desktop = f
        .store
        .android_desktop_start(&f.owner, run, 640, 480)
        .await
        .unwrap();
    let desktop = id(&desktop, "id");
    let ticket = token("pg-view-ticket");
    f.store
        .android_desktop_ticket(&viewer, run, desktop, false, None, &ticket)
        .await
        .unwrap();
    let view = f
        .store
        .android_desktop_consume(desktop, &ticket)
        .await
        .unwrap();
    assert_eq!(view.session, viewer.session_id());
    assert_ne!(view.session, f.owner.session_id());
    let hub = f
        .store
        .android_hub_claim(&f.agent, Uuid::new_v4(), Uuid::new_v4(), None)
        .await
        .unwrap();
    f.permissions(&viewer, 1).await;
    f.store.android_desktop_access_current(&view).await.unwrap();
    f.store
        .android_desktop_agent_current(&f.agent, &view)
        .await
        .unwrap();
    assert_eq!(f.store.android_hub_owner(&view).await.unwrap(), hub);
    let pending = token("pg-view-forward");
    f.store
        .android_hub_forward(&hub, &view, None, None, &pending)
        .await
        .unwrap();
    let changed =
        sqlx::query("UPDATE identity_session_scopes SET organization_id=$1 WHERE session_id=$2")
            .bind(other_org.id.to_string())
            .bind(viewer.session_id().to_string())
            .execute(&f.pool)
            .await
            .unwrap();
    assert_eq!(changed.rows_affected(), 1);
    let before = f.product_state().await;
    f.denied_access(&view, &hub, &pending).await;
    assert_eq!(f.product_state().await, before);
    f.assert_original_bytes(run, lease).await;

    // A different genuine original session obtains actual reauth, control ticket
    // and current access. No replacement generation is introduced after consume.
    let reauth = token("pg-control-reauth");
    f.store
        .android_reauthentication_grant(&controller, &reauth)
        .await
        .unwrap();
    let ticket = token("pg-control-ticket");
    f.store
        .android_desktop_ticket(&controller, run, desktop, true, Some(&reauth), &ticket)
        .await
        .unwrap();
    let control = f
        .store
        .android_desktop_consume(desktop, &ticket)
        .await
        .unwrap();
    assert!(control.control);
    assert_eq!(control.session, controller.session_id());
    assert_ne!(control.session, f.owner.session_id());
    f.store
        .android_desktop_access_current(&control)
        .await
        .unwrap();
    f.store
        .android_desktop_agent_current(&f.agent, &control)
        .await
        .unwrap();
    f.store.android_hub_renew(&f.agent, &hub).await.unwrap();
    let pending = token("pg-control-forward");
    f.store
        .android_hub_forward(&hub, &control, None, None, &pending)
        .await
        .unwrap();
    f.permissions(&controller, 1).await;
    let before = f.product_state().await;
    f.denied_access(&control, &hub, &pending).await;
    assert_eq!(f.product_state().await, before);
    f.assert_original_bytes(run, lease).await;
    f.close().await;
}
