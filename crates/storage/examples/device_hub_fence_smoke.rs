//! One local SQLite authority check. No sockets, tools, deployment or real keys.
use cowork_contracts::{DeviceCapability, DeviceExecutorRegistration, PairDeviceExecutor};
use cowork_domain::Error;
use cowork_storage::{AndroidDeviceRunRequest, Store};
use serde_json::json;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scratch = tempfile::tempdir()?;
    let store = Store::sqlite(&scratch.path().join("hub.db")).await?;
    let identity = store.identity();
    let bootstrap = "synthetic-bootstrap-credential-only-for-local-smoke";
    identity
        .initialize_credentials(
            bootstrap,
            "synthetic-settlement-credential-only-for-local-smoke",
        )
        .await
        .map_err(|e| format!("initialize fixture credentials: {e:?}"))?;
    let (_, session) = identity
        .bootstrap(
            bootstrap,
            "hub-owner",
            "Hub owner",
            "Synthetic-password-only-for-local-smoke!",
            "Hub tenant",
        )
        .await
        .map_err(|e| format!("bootstrap fixture identity: {e:?}"))?;
    let actor = identity
        .authenticate_session(&session.token)
        .await
        .map_err(|e| format!("authenticate fixture identity: {e:?}"))?;
    let id = Uuid::new_v4();
    store
        .android_pair_device(
            &actor,
            PairDeviceExecutor {
                team_id: None,
                registration: DeviceExecutorRegistration {
                    schema_version: 2,
                    executor_id: id,
                    kind: "personal_device".into(),
                    pool_id: None,
                    owner_user_id: None,
                    display_name: "SQLite fixture device".into(),
                    protocol_version: 2,
                    capabilities: vec![DeviceCapability {
                        schema_version: 2,
                        name: "desktop.windows".into(),
                        version: "fixture".into(),
                        attributes: Default::default(),
                    }],
                    labels: Default::default(),
                    personal_device_remote_control: Some("unattended".into()),
                    max_concurrent_runs: 1,
                },
            },
        )
        .await?;
    let token = "synthetic-agent-credential-0000000000000000000000000000000000";
    let issued = store
        .android_create_device_credential(&actor, id, "Fixture", None, token)
        .await?;
    let credential: Uuid = issued["credential_id"].as_str().unwrap().parse()?;
    let a = store.android_agent_authenticate(id, token).await?;
    let first = store
        .android_hub_claim(
            &a,
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("https://replica-a.internal:43822/"),
        )
        .await?;
    assert!(matches!(
        store
            .android_hub_claim(
                &a,
                Uuid::new_v4(),
                Uuid::new_v4(),
                Some("https://replica-b.internal:43822/")
            )
            .await,
        Err(Error::Conflict)
    ));
    store.android_hub_renew(&a, &first).await?;
    store.android_hub_release(&first).await?;
    let second = store
        .android_hub_claim(
            &a,
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("https://replica-b.internal:43822/"),
        )
        .await?;
    assert_eq!(second.epoch, first.epoch + 1);
    assert_ne!(first.nonce, second.nonce);
    assert!(matches!(
        store.android_hub_renew(&a, &first).await,
        Err(Error::Conflict)
    ));
    store.android_hub_release(&first).await?; // A late old process cannot erase the new owner.
    store.android_hub_current(&second).await?;
    assert!(matches!(
        store
            .android_hub_take_forward(
                second.node,
                second.boot,
                "synthetic-forged-forward-ticket-00000000000000000000",
                "desktop"
            )
            .await,
        Err(Error::Forbidden)
    ));
    // A real actor-owned project, run lease and original one-use desktop ticket
    // establish authority. No external executor or desktop process is started.
    let project = store
        .android_personal_create_project(
            &actor,
            json!({"privacy":"private_local","name":"Hub source","description":"","policy":{}}),
        )
        .await?;
    let project_id: Uuid = project["id"].as_str().unwrap().parse()?;
    let thread = store
        .android_personal_create_thread(
            &actor,
            json!({"project_id":project_id,"title":"Hub grant"}),
            None,
        )
        .await?;
    let thread_id: Uuid = thread["id"].as_str().unwrap().parse()?;
    let run = store
        .android_device_create_run(
            &actor,
            AndroidDeviceRunRequest {
                team: None,
                project: project_id,
                order: thread_id,
                project_revision: 1,
                target: json!({"kind":"personal_device","device_id":id}),
                capabilities: vec!["desktop.windows".into()],
                input: json!({"prompt":"Fixture only"}),
                model_profile: None,
                snapshot: None,
                key: "hub-fixture".into(),
                task: None,
            },
        )
        .await?;
    let run_id: Uuid = run["spec"]["id"].as_str().unwrap().parse()?;
    let _lease = store
        .android_device_claim(&a)
        .await?
        .expect("actual admitted device lease");
    let desktop = store
        .android_desktop_start(&actor, run_id, 640, 480)
        .await?;
    let desktop_id: Uuid = desktop["id"].as_str().unwrap().parse()?;
    let ticket = "synthetic-desktop-ticket-0000000000000000000000000000";
    store
        .android_desktop_ticket(&actor, run_id, desktop_id, false, None, ticket)
        .await?;
    let access = store.android_desktop_consume(desktop_id, ticket).await?;
    let forward = "synthetic-valid-forward-ticket-000000000000000000000000";
    store
        .android_hub_forward(&second, &access, None, None, forward)
        .await?;
    assert!(matches!(
        store
            .android_hub_take_forward(first.node, first.boot, forward, "desktop")
            .await,
        Err(Error::Conflict)
    ));
    let admitted = store
        .android_hub_take_forward(second.node, second.boot, forward, "desktop")
        .await?;
    assert_eq!(admitted.access.identity, actor.identity_id());
    assert_eq!(admitted.access.run, run_id);
    assert!(matches!(
        store
            .android_hub_take_forward(second.node, second.boot, forward, "desktop")
            .await,
        Err(Error::Forbidden)
    ));
    let revoked_forward = "synthetic-revoked-forward-ticket-000000000000000000000";
    store
        .android_hub_forward(&second, &access, None, None, revoked_forward)
        .await?;
    store
        .android_revoke_device_credential(&actor, id, credential)
        .await?;
    assert!(matches!(
        store.android_hub_current(&second).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .android_hub_take_forward(second.node, second.boot, revoked_forward, "desktop")
            .await,
        Err(Error::Forbidden)
    ));
    store.close().await;
    println!(
        "PASS: actual SQLite owner fencing, live private-project/run authority, one-use purpose/owner grants and credential revocation"
    );
    Ok(())
}
