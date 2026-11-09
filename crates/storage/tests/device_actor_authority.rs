//! Synthetic SQLite Store/Identity regressions. No runner, network, tool or real key.
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
use sqlx::{AnyPool, Row, any::AnyPoolOptions};
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
        Err(other) => panic!("Expected Forbidden, received a different error: {other:?}"),
        Ok(_) => panic!("Operation succeeded after its actor authority changed"),
    }
}

/// Logical original rows, including every column/NULL/blob value of the selected
/// product tables. Clock samples and the deliberately changed Identity rows are
/// excluded. SQLite quote() returns text and avoids Any's nullable-type guessing.
type ProductState = BTreeMap<String, Vec<Vec<String>>>;

struct Fixture {
    directory: tempfile::TempDir,
    store: Store,
    pool: AnyPool,
    service: TeamService,
    owner: Principal,
    team: Uuid,
    project: Uuid,
    order: Uuid,
    device: Uuid,
    credential: Uuid,
    agent: AndroidAgentPrincipal,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("device-authority.sqlite");
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
                "device-actor-owner",
                "Device actor owner",
                "SyntheticDeviceActorPassword-2026!",
                "Device actor team",
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
                    name: "Device authority project".into(),
                    idempotency_key: "device-project".into(),
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
                    title: "Device authority order".into(),
                    prompt: "Synthetic input only".into(),
                    idempotency_key: "device-order".into(),
                },
            )
            .await
            .unwrap()
            .id;
        let device = Uuid::new_v4();
        // Enrollment/credentials use the genuine unscoped original session.
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
                        display_name: "Synthetic paired device".into(),
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
        let issued = store
            .android_create_device_credential(
                &owner,
                device,
                "Synthetic fixture",
                None,
                AGENT_TOKEN,
            )
            .await
            .unwrap();
        let credential = id(&issued, "credential_id");
        let agent = store
            .android_agent_authenticate(device, AGENT_TOKEN)
            .await
            .unwrap();
        Self {
            directory,
            store,
            pool,
            service,
            owner,
            team,
            project,
            order,
            device,
            credential,
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
            let columns = sqlx::query(&format!("PRAGMA table_info(\"{table}\")"))
                .fetch_all(&self.pool)
                .await
                .unwrap();
            assert!(!columns.is_empty(), "Missing fixture table {table}");
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
            .fetch_all(&self.pool)
            .await
            .unwrap();
            let values = rows
                .iter()
                .map(|row| {
                    (0..columns.len())
                        .map(|n| row.try_get::<String, _>(n).unwrap())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
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
            directory,
            store,
            pool,
            service,
            ..
        } = self;
        drop(service);
        store.close().await;
        pool.close().await;
        drop(store);
        drop(pool);
        directory.close().unwrap();
    }
}

#[tokio::test]
async fn scoped_creator_claims_heartbeats_and_reads_exact_original_snapshot_bytes() {
    let f = Fixture::new().await;
    let creator = f.scoped().await;
    let original = f.create_run(&creator).await;
    let run = run_id(&original);
    let lease = f.claim(&original).await;
    let heartbeat = f
        .store
        .android_device_lease_heartbeat(&f.agent, run, lease)
        .await
        .unwrap();
    assert_eq!(id(&heartbeat, "lease_token"), lease);
    assert_eq!(heartbeat["run"]["spec"], original["spec"]);
    f.assert_original_bytes(run, lease).await;
    f.close().await;
}

#[tokio::test]
async fn creator_readonly_after_queue_interrupts_without_a_lease_or_lease_event() {
    let f = Fixture::new().await;
    let creator = f.scoped().await;
    let original = f.create_run(&creator).await;
    let run = run_id(&original);
    let mut expected_state = f.product_state().await;
    assert_eq!(expected_state["android_scoped_runs"].len(), 1);
    f.permissions(&creator, 1).await;
    assert!(
        f.store
            .android_device_claim(&f.agent)
            .await
            .unwrap()
            .is_none()
    );
    let row = sqlx::query("SELECT * FROM android_scoped_runs WHERE id=$1")
        .bind(run.to_string())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(row.try_get::<String, _>("state").unwrap(), "interrupted");
    let revision: i64 = row.try_get("revision").unwrap();
    assert_eq!(revision, original["revision"].as_i64().unwrap() + 1);
    let updated_at: i64 = row.try_get("updated_at").unwrap();
    let old_updated_at =
        chrono::DateTime::parse_from_rfc3339(original["updated_at"].as_str().unwrap())
            .unwrap()
            .timestamp_millis();
    assert!(updated_at >= old_updated_at);
    let value: Value = serde_json::from_str(&row.try_get::<String, _>("payload").unwrap()).unwrap();
    let payload_time = chrono::DateTime::parse_from_rfc3339(value["updated_at"].as_str().unwrap())
        .unwrap()
        .timestamp_millis();
    assert_eq!(payload_time, updated_at);
    let mut expected_payload = original.clone();
    expected_payload["state"] = json!("interrupted");
    expected_payload["revision"] = json!(revision);
    expected_payload["etag"] = json!(format!("{run}:{revision}"));
    expected_payload["updated_at"] = value["updated_at"].clone();
    expected_payload["error"] = json!({"code":"device_authority_changed","message":"Execution authority changed","retryable":false});
    assert_eq!(value, expected_payload);
    let after = f.product_state().await;
    assert_eq!(after["android_scoped_runs"].len(), 1);
    let columns = sqlx::query("PRAGMA table_info(android_scoped_runs)")
        .fetch_all(&f.pool)
        .await
        .unwrap();
    // Permit precisely the four columns save_run intentionally updates in this
    // one original row. Every original binding, NULL and other table remains.
    for column in ["payload", "state", "revision", "updated_at"] {
        let index = columns
            .iter()
            .position(|r| r.try_get::<String, _>("name").unwrap() == column)
            .unwrap();
        expected_state.get_mut("android_scoped_runs").unwrap()[0][index] =
            after["android_scoped_runs"][0][index].clone();
    }
    assert_eq!(
        after, expected_state,
        "Queue rejection changed an original binding or another product row"
    );
    drop(row);
    f.close().await;
}

#[tokio::test]
async fn creator_readonly_after_claim_cannot_renew_append_or_read_and_keeps_original_rows() {
    let f = Fixture::new().await;
    let creator = f.scoped().await;
    let original = f.create_run(&creator).await;
    let run = run_id(&original);
    let lease = f.claim(&original).await;
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
    f.permissions(&creator, 1).await;
    let before = f.product_state().await;
    f.denied_lease_operations(run, lease).await;
    assert_eq!(f.product_state().await, before);
    f.close().await;
}

#[tokio::test]
async fn identity_clock_fence_blocks_claim_and_rolls_back_the_unchanged_queue() {
    let f = Fixture::new().await;
    let creator = f.scoped().await;
    let _original = f.create_run(&creator).await;
    let before = f.product_state().await;
    let changed = sqlx::query(
        "UPDATE identity_installation SET utc_highwater=utc_highwater+3600000 WHERE id=1",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(changed.rows_affected(), 1);
    // The actual system/database clock and separate device highwater are untouched.
    assert!(f.store.android_device_claim(&f.agent).await.is_err());
    assert_eq!(f.product_state().await, before);
    f.close().await;
}

#[tokio::test]
async fn real_session_logout_and_real_device_credential_revocation_block_existing_leases() {
    for revoke_credential in [false, true] {
        let f = Fixture::new().await;
        let creator = f.scoped().await;
        let original = f.create_run(&creator).await;
        let run = run_id(&original);
        let lease = f.claim(&original).await;
        f.assert_original_bytes(run, lease).await;
        if revoke_credential {
            f.store
                .android_revoke_device_credential(&f.owner, f.device, f.credential)
                .await
                .unwrap();
            forbidden(
                f.store
                    .android_agent_authenticate(f.device, AGENT_TOKEN)
                    .await,
            );
        } else {
            f.store.identity().logout(&creator).await.unwrap();
        }
        let before = f.product_state().await;
        f.denied_lease_operations(run, lease).await;
        assert_eq!(f.product_state().await, before);
        f.close().await;
    }
}

#[tokio::test]
async fn separate_readonly_desktop_session_can_view_but_cannot_switch_its_real_org_scope() {
    let f = Fixture::new().await;
    let foreign = f
        .store
        .identity()
        .create_organization(
            &f.owner,
            "Other actual fixture organization",
            "other-organization",
        )
        .await
        .unwrap();
    let viewer = f.scoped().await;
    // Creator remains the unchanged genuine unscoped original session.
    let original = f.create_run(&f.owner).await;
    let lease = f.claim(&original).await;
    let run = run_id(&original);
    let desktop = f
        .store
        .android_desktop_start(&f.owner, run, 640, 480)
        .await
        .unwrap();
    let desktop = id(&desktop, "id");
    let ticket = token("view-ticket");
    f.store
        .android_desktop_ticket(&viewer, run, desktop, false, None, &ticket)
        .await
        .unwrap();
    let access = f
        .store
        .android_desktop_consume(desktop, &ticket)
        .await
        .unwrap();
    assert_eq!(access.session, viewer.session_id());
    assert_ne!(access.session, f.owner.session_id());
    let hub = f
        .store
        .android_hub_claim(&f.agent, Uuid::new_v4(), Uuid::new_v4(), None)
        .await
        .unwrap();
    f.permissions(&viewer, 1).await;
    f.store
        .android_desktop_access_current(&access)
        .await
        .unwrap();
    f.store
        .android_desktop_agent_current(&f.agent, &access)
        .await
        .unwrap();
    assert_eq!(f.store.android_hub_owner(&access).await.unwrap(), hub);
    let pending = token("pending-view-forward");
    f.store
        .android_hub_forward(&hub, &access, None, None, &pending)
        .await
        .unwrap();
    let changed =
        sqlx::query("UPDATE identity_session_scopes SET organization_id=$1 WHERE session_id=$2")
            .bind(foreign.id.to_string())
            .bind(viewer.session_id().to_string())
            .execute(&f.pool)
            .await
            .unwrap();
    assert_eq!(changed.rows_affected(), 1);
    let before = f.product_state().await;
    f.denied_access(&access, &hub, &pending).await;
    assert_eq!(f.product_state().await, before);
    // The original creator/device/snapshot stay usable: this is the OTHER actor gate.
    f.assert_original_bytes(run, lease).await;
    f.close().await;
}

#[tokio::test]
async fn separate_controller_readonly_ceiling_blocks_original_access_without_mutating_tickets() {
    let f = Fixture::new().await;
    let controller = f.scoped().await;
    let original = f.create_run(&f.owner).await;
    let run = run_id(&original);
    let lease = f.claim(&original).await;
    let desktop = f
        .store
        .android_desktop_start(&f.owner, run, 640, 480)
        .await
        .unwrap();
    let desktop = id(&desktop, "id");
    let reauth = token("control-reauth");
    f.store
        .android_reauthentication_grant(&controller, &reauth)
        .await
        .unwrap();
    let ticket = token("control-ticket");
    f.store
        .android_desktop_ticket(&controller, run, desktop, true, Some(&reauth), &ticket)
        .await
        .unwrap();
    let access = f
        .store
        .android_desktop_consume(desktop, &ticket)
        .await
        .unwrap();
    assert!(access.control);
    assert_eq!(access.session, controller.session_id());
    assert_ne!(access.session, f.owner.session_id());
    f.store
        .android_desktop_access_current(&access)
        .await
        .unwrap();
    f.store
        .android_desktop_agent_current(&f.agent, &access)
        .await
        .unwrap();
    let hub = f
        .store
        .android_hub_claim(&f.agent, Uuid::new_v4(), Uuid::new_v4(), None)
        .await
        .unwrap();
    let pending = token("pending-control-forward");
    f.store
        .android_hub_forward(&hub, &access, None, None, &pending)
        .await
        .unwrap();
    // Preserve the authentic consumed reauthentication/stream tickets and the
    // still-unconsumed Hub forward. No newer controller generation is created.
    f.permissions(&controller, 1).await;
    let before = f.product_state().await;
    f.denied_access(&access, &hub, &pending).await;
    assert_eq!(f.product_state().await, before);
    f.assert_original_bytes(run, lease).await;
    f.close().await;
}

#[tokio::test]
async fn private_owner_tombstones_do_not_restore_content_or_revoked_authority() {
    use cowork_storage::AndroidWireSyncChange;

    fn wire_change(
        device: Uuid,
        kind: &str,
        entity: Uuid,
        revision: i64,
        payload: Option<Value>,
    ) -> AndroidWireSyncChange {
        AndroidWireSyncChange {
            schema_version: 2,
            operation_id: Uuid::new_v4(),
            device_id: device,
            entity_type: kind.into(),
            entity_id: entity,
            base_revision: revision,
            operation: if payload.is_some() {
                "upsert"
            } else {
                "delete"
            }
            .into(),
            payload,
            client_timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    async fn no_private_targets(
        store: &Store,
        actor: &Principal,
        targets: &[(&str, Uuid)],
        cursors: &[i64],
    ) {
        let feed = store.android_wire_sync_pull(actor, 0, 100).await.unwrap();
        assert_eq!(feed["has_more"], false);
        for &(kind, entity) in targets {
            assert!(
                !feed["changes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v["entity_type"] == kind && v["entity_id"] == json!(entity))
            );
            let page = store
                .android_wire_sync_entities(actor, kind, None, None, 100)
                .await
                .unwrap();
            assert_eq!(page["has_more"], false);
            assert!(
                !page["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v["entity_id"] == json!(entity))
            );
        }
        for &cursor in cursors {
            assert!(
                store
                    .android_wire_sync_frame(actor, cursor)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }

    // Only this fresh SQLite file is used; no device/run/credential/key fixture.
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("private-tombstones.sqlite");
    let store = Store::sqlite(&database).await.unwrap();
    let identity = store.identity();
    let bootstrap = token("private-tombstone-bootstrap");
    let owner_password = format!("Synthetic-tombstone-owner-{}-A7!", Uuid::new_v4());
    let other_password = format!("Synthetic-tombstone-viewer-{}-A7!", Uuid::new_v4());
    identity
        .initialize_credentials(&bootstrap, &token("private-tombstone-settlement"))
        .await
        .unwrap();
    let (organization, _) = identity
        .bootstrap(
            &bootstrap,
            "tombstone-owner",
            "Synthetic tombstone owner",
            &owner_password,
            "Synthetic tombstone organization",
        )
        .await
        .unwrap();
    let owner_session = identity
        .login("tombstone-owner", &owner_password, 3_600_000)
        .await
        .unwrap();
    let owner = identity
        .authenticate_session(&owner_session.token)
        .await
        .unwrap();
    identity
        .register_local_identity(
            "tombstone-viewer",
            "Synthetic tombstone viewer",
            &other_password,
        )
        .await
        .unwrap();
    let other_session = identity
        .login("tombstone-viewer", &other_password, 3_600_000)
        .await
        .unwrap();
    let other = identity
        .authenticate_session(&other_session.token)
        .await
        .unwrap();
    let device = Uuid::new_v4();
    store
        .android_wire_bind_device(&owner, device)
        .await
        .unwrap();
    let project = Uuid::new_v4();
    let thread = Uuid::new_v4();
    let task = Uuid::new_v4();
    let message = Uuid::new_v4();
    let targets = [
        ("project", project),
        ("thread", thread),
        ("task", task),
        ("message", message),
    ];
    for change in [
        wire_change(
            device,
            "project",
            project,
            0,
            Some(json!({
                "title":"Private metadata fixture", "instructions":"Inert original",
                "project_kind":"private", "thread_ids":[thread]
            })),
        ),
        wire_change(
            device,
            "thread",
            thread,
            0,
            Some(json!({
                "project_id":project, "title":"Inert thread"
            })),
        ),
        wire_change(
            device,
            "task",
            task,
            0,
            Some(json!({
                "project_id":project, "thread_id":thread, "title":"Inert task",
                "description":"No execution is requested", "runner":"model"
            })),
        ),
        wire_change(
            device,
            "message",
            message,
            0,
            Some(json!({
                "project_id":project, "thread_id":thread, "role":"assistant",
                "content":"Synthetic inert history", "timestamp":1234567890000i64
            })),
        ),
    ] {
        let receipt = store
            .android_wire_sync_push(&owner, vec![change])
            .await
            .unwrap();
        assert_eq!(receipt["results"].as_array().unwrap().len(), 1);
        assert_eq!(receipt["results"][0]["status"], "applied");
    }

    let before = store.android_wire_sync_pull(&owner, 0, 100).await.unwrap();
    assert_eq!(before["has_more"], false);
    let project_upsert = before["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| {
            v["entity_type"] == "project"
                && v["entity_id"] == json!(project)
                && v["operation"] == "upsert"
        })
        .expect("The project-bound original upsert must be visible before delete");
    let upsert_cursor = project_upsert["cursor"].as_i64().unwrap();
    assert_eq!(
        store
            .android_wire_sync_frame(&owner, upsert_cursor)
            .await
            .unwrap(),
        Some(project_upsert.clone())
    );

    // Real grant, positive read control, then revocation using its actual revision.
    no_private_targets(&store, &other, &targets, &[upsert_cursor]).await;
    let grant = store
        .android_private_project_member_write(
            &owner,
            project,
            other.identity_id(),
            Some("viewer"),
            Some(0),
            Some("tombstone-viewer-grant"),
        )
        .await
        .unwrap();
    assert_eq!(grant["active"], true);
    let shared = store.android_wire_sync_pull(&other, 0, 100).await.unwrap();
    assert!(
        shared["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["entity_type"] == "project"
                && v["entity_id"] == json!(project)
                && v["operation"] == "upsert")
    );
    assert_eq!(
        store
            .android_wire_sync_frame(&other, upsert_cursor)
            .await
            .unwrap(),
        Some(project_upsert.clone())
    );
    let shared_page = store
        .android_wire_sync_entities(&other, "project", None, None, 100)
        .await
        .unwrap();
    assert!(
        shared_page["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["entity_id"] == json!(project))
    );
    let revoked = store
        .android_private_project_member_write(
            &owner,
            project,
            other.identity_id(),
            None,
            Some(grant["revision"].as_i64().unwrap()),
            Some("tombstone-viewer-revoke"),
        )
        .await
        .unwrap();
    assert_eq!(revoked["active"], false);
    no_private_targets(&store, &other, &targets, &[upsert_cursor]).await;

    // Use the existing verified scoped-session hook in a genuine caller TX.
    // No Principal, membership, grant, session row or permission is fabricated.
    let encoded = percent_encoding::utf8_percent_encode(
        database.to_str().unwrap(),
        percent_encoding::NON_ALPHANUMERIC,
    )
    .to_string();
    let pool = AnyPoolOptions::new()
        .max_connections(2)
        .connect(&format!("sqlite:{encoded}?mode=rw"))
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let scoped_session = identity
        .issue_verified_scoped_session_with_permissions_in_transaction(
            &mut tx,
            owner.identity_id(),
            organization.id,
            3_600_000,
            15,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let scoped_owner = identity
        .authenticate_session(&scoped_session.token)
        .await
        .unwrap();
    assert_eq!(scoped_owner.identity_id(), owner.identity_id());
    assert_ne!(scoped_owner.session_id(), owner.session_id());
    no_private_targets(&store, &scoped_owner, &targets, &[upsert_cursor]).await;

    let current = store
        .android_personal_project(&owner, project)
        .await
        .unwrap()
        .unwrap();
    let revision = current["revision"].as_i64().unwrap();
    assert_eq!(project_upsert["revision"], revision);
    let deleted = store
        .android_wire_sync_push(
            &owner,
            vec![wire_change(device, "project", project, revision, None)],
        )
        .await
        .unwrap();
    assert_eq!(deleted["results"].as_array().unwrap().len(), 1);
    assert_eq!(deleted["results"][0]["status"], "applied");
    assert_eq!(deleted["results"][0]["entity"]["revision"], revision + 1);
    assert_eq!(deleted["results"][0]["entity"]["tombstone"], true);
    assert_eq!(
        deleted["results"][0]["entity"].get("payload"),
        Some(&Value::Null)
    );

    let feed = store.android_wire_sync_pull(&owner, 0, 100).await.unwrap();
    assert_eq!(feed["has_more"], false);
    let mut delete_cursors = Vec::new();
    for &(kind, entity) in &targets {
        let change = feed["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| {
                v["entity_type"] == kind
                    && v["entity_id"] == json!(entity)
                    && v["operation"] == "delete"
            })
            .expect("Each own private tombstone must be delivered");
        assert_eq!(change.get("payload"), Some(&Value::Null));
        let cursor = change["cursor"].as_i64().unwrap();
        delete_cursors.push(cursor);
        assert_eq!(
            store.android_wire_sync_frame(&owner, cursor).await.unwrap(),
            Some(change.clone())
        );
        let page = store
            .android_wire_sync_entities(&owner, kind, None, None, 100)
            .await
            .unwrap();
        assert_eq!(page["has_more"], false);
        let item = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["entity_id"] == json!(entity))
            .unwrap();
        assert_eq!(item["tombstone"], true);
        assert_eq!(item.get("payload"), Some(&Value::Null));
    }
    // Only the known project-bound upsert is checked; no orphan-history premise.
    assert!(
        store
            .android_wire_sync_frame(&owner, upsert_cursor)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !feed["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["entity_type"] == "project"
                && v["entity_id"] == json!(project)
                && v["operation"] == "upsert")
    );
    let mut all_cursors = delete_cursors.clone();
    all_cursors.push(upsert_cursor);
    no_private_targets(&store, &other, &targets, &all_cursors).await;
    no_private_targets(&store, &scoped_owner, &targets, &all_cursors).await;

    identity.logout(&owner).await.unwrap();
    forbidden(store.android_wire_sync_pull(&owner, 0, 100).await);
    for &cursor in &delete_cursors {
        forbidden(store.android_wire_sync_frame(&owner, cursor).await);
    }
    for &(kind, _) in &targets {
        forbidden(
            store
                .android_wire_sync_entities(&owner, kind, None, None, 100)
                .await,
        );
    }
    drop(identity);
    pool.close().await;
    store.close().await;
    drop(pool);
    drop(store);
    directory.close().unwrap();
}
