#[path = "support/mcp.rs"]
mod support;
use cowork_contracts::*;
use cowork_domain::*;
use cowork_mcp::{Endpoint, Limits, SessionCipher};
use cowork_storage::*;
use std::{sync::atomic::Ordering, time::Duration};
use support::*;
use zeroize::Zeroizing;

async fn shared(store: &Store) {
    let dir = tempfile::tempdir().unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    let f = setup(store, &remote, AccessMode::Autonomous).await;
    let cipher = SessionCipher::new("fixture", Zeroizing::new([7; 32])).unwrap();
    let (a, b) = tokio::join!(
        store.begin_mcp_connection(&f.run.id, &f.token, &f.server.id),
        store.begin_mcp_connection(&f.run.id, &f.token, &f.server.id)
    );
    let permits = [a.unwrap(), b.unwrap()]
        .into_iter()
        .filter_map(|a| match a {
            McpConnectionAdmission::Connect(p) => Some(p),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(permits.len(), 1);
    let permit = permits.into_iter().next().unwrap();
    let mut session = remote.session().await;
    let scope = permit.record().scope(&f.run, 2).unwrap();
    let sealed = cipher.seal(&session, &scope).unwrap();
    let ready = store
        .finish_mcp_connection(
            &f.run.id,
            &f.token,
            permit,
            session.connection_id(),
            session.epoch(),
            sealed.clone(),
        )
        .await
        .unwrap();
    assert!(ready.state == McpConnectionState::Ready);
    let McpAdmission::Dispatch(permit) = store
        .admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&session, &f))
        .await
        .unwrap()
    else {
        panic!("permit")
    };
    let busy = store
        .mcp_connection(&f.run.id, &f.server.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        busy.state
            == McpConnectionState::Busy {
                tool_call_id: f.tool.id.clone()
            }
    );
    session.call(permit.into_prepared()).await.unwrap();
    // A receipt alone must never release a session using its old checkpoint.
    assert!(matches!(
        store
            .finish_mcp(&f.run.id, &f.token, &f.tool.id, result())
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(store.stored_run(&f.run.id).await.unwrap().record.steps, 0);
    let next_sealed = cipher
        .seal(
            &session,
            &busy.scope(&f.run, busy.next_revision().unwrap()).unwrap(),
        )
        .unwrap();
    store
        .finish_mcp_session(
            &f.run.id,
            &f.token,
            &f.tool.id,
            result(),
            McpSessionCompletion::Ready {
                record_id: busy.id,
                expected_revision: busy.revision,
                connection_id: session.connection_id(),
                epoch: session.epoch(),
                sealed: next_sealed,
            },
        )
        .await
        .unwrap();
    let ready = store
        .mcp_connection(&f.run.id, &f.server.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.revision, 4);
    assert_eq!(ready.epoch, Some(2));
    // Old ciphertext is authentic, but its epoch can no longer obtain a dispatch.
    let stale = cipher
        .open(
            &sealed,
            &scope,
            Endpoint::new(&remote.endpoint, None).unwrap(),
            Limits::default(),
            vec![definition()],
        )
        .unwrap();
    let mut next = f.tool.clone();
    next.id = id();
    store
        .mutate(&f.run.id, Some(&f.token), |r| {
            r.checkpoint.pending_tools = vec![next.clone()];
            Ok(Change::default())
        })
        .await
        .unwrap();
    assert!(matches!(
        store
            .admit_mcp(
                &f.run.id,
                &f.token,
                &next,
                stale
                    .prepare(
                        &f.server.catalog_sha256,
                        "append_note",
                        next.arguments.clone()
                    )
                    .unwrap()
            )
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(store.mcp_attempts(&f.run.id).await.unwrap().len(), 1);
    let archive = store.export_order(&f.run.spec.order_id).await.unwrap();
    let exported = serde_json::to_string(&archive).unwrap();
    assert!(!exported.contains("ciphertext"));
    assert!(!exported.contains(&remote.state.session_ids.lock().unwrap()[0]));
    assert_eq!(archive.sha256, canonical_hash(&archive.data));
    assert_eq!(remote.state.initializations.load(Ordering::SeqCst), 1);
    assert_eq!(remote.calls(), 1);
}
#[tokio::test]
async fn sqlite_session_admission_checkpoint_completion_and_stale_replay() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("db")).await.unwrap();
    shared(&store).await;
}
#[tokio::test]
#[ignore = "Requires a dedicated PostgreSQL database in COWORK_TEST_POSTGRES"]
async fn postgres_mcp_connection_contract() {
    let store = Store::connect(&std::env::var("COWORK_TEST_POSTGRES").unwrap())
        .await
        .unwrap();
    shared(&store).await;
}
