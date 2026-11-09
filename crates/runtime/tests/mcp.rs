#[path = "../../storage/tests/support/mcp.rs"]
mod support;
use cowork_contracts::*;
use cowork_domain::Error;
use cowork_runtime::Runtime;
use cowork_storage::Store;
use std::time::Duration;
use support::*;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn shared_approval_broker_dispatches_exactly_once_and_replays_only_the_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("db")).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    let mut f = setup(&store, &remote, AccessMode::ConfirmMutations).await;
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    let mut session = remote.session().await;
    let stop = CancellationToken::new();
    assert!(matches!(
        runtime
            .dispatch_mcp_tool(&f.run.id, &f.token, &f.tool, &mut session, &stop)
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(remote.calls(), 0);
    approve(&store, &mut f).await;
    let completed = runtime
        .dispatch_mcp_tool(&f.run.id, &f.token, &f.tool, &mut session, &stop)
        .await
        .unwrap();
    assert!(matches!(completed.state, McpAttemptState::Completed { .. }));
    assert_eq!(
        runtime
            .dispatch_mcp_tool(&f.run.id, &f.token, &f.tool, &mut session, &stop)
            .await
            .unwrap(),
        completed
    );
    assert_eq!(remote.calls(), 1);
}
#[tokio::test]
async fn tool_error_after_effect_interrupts_the_run_and_keeps_the_protected_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("db")).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, true).await;
    let f = setup(&store, &remote, AccessMode::Autonomous).await;
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    let mut session = remote.session().await;
    assert!(matches!(
        runtime
            .dispatch_mcp_tool(
                &f.run.id,
                &f.token,
                &f.tool,
                &mut session,
                &CancellationToken::new()
            )
            .await,
        Err(Error::UncertainEffect)
    ));
    let detail = store.detail(&f.run.id).await.unwrap();
    assert_eq!(detail.run.state, RunState::Interrupted);
    assert!(matches!(
        &detail.mcp_attempts[0].state,
        McpAttemptState::OutcomeUncertain {
            response: Some(_),
            ..
        }
    ));
    assert!(store.resume(&f.run.id).await.is_err());
    assert_eq!(remote.calls(), 1);
    assert!(!session.is_usable());
}
#[tokio::test]
async fn cancellation_during_remote_mutation_never_claims_remote_termination() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("db")).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::from_secs(10), false).await;
    let f = setup(&store, &remote, AccessMode::Autonomous).await;
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    let mut session = remote.session().await;
    let stop = CancellationToken::new();
    let dispatch = runtime.dispatch_mcp_tool(&f.run.id, &f.token, &f.tool, &mut session, &stop);
    let cancel = async {
        let start = tokio::time::Instant::now();
        while !remote.state.path.exists() {
            assert!(start.elapsed() < Duration::from_secs(3));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            store.cancel(&f.run.id).await.unwrap().state,
            RunState::CancelRequested
        );
    };
    let (result, _) = tokio::join!(dispatch, cancel);
    assert!(matches!(result, Err(Error::UncertainEffect)));
    assert_eq!(
        store.detail(&f.run.id).await.unwrap().run.state,
        RunState::Interrupted
    );
    assert!(
        store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .checkpoint
            .in_flight
            .is_some()
    );
    assert!(!session.is_usable());
    assert_eq!(remote.calls(), 1);
}

#[tokio::test]
#[ignore = "Synthetic child killed by hard_crash_after_remote_effect_retains_the_dispatch_marker"]
async fn mcp_crash_fixture() {
    let root = std::path::PathBuf::from(std::env::var_os("COWORK_MCP_CRASH").unwrap());
    let data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fixture.json")).unwrap()).unwrap();
    let store = Store::sqlite(&root.join("db")).await.unwrap();
    let runtime = Runtime::new(store, vec![], false).unwrap();
    let tool: ToolCall = serde_json::from_value(data["tool"].clone()).unwrap();
    let mut session = cowork_mcp::Session::connect(
        cowork_mcp::Endpoint::new(data["endpoint"].as_str().unwrap(), None).unwrap(),
        cowork_mcp::Limits {
            timeout_ms: 10_000,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    session.discover().await.unwrap();
    runtime
        .dispatch_mcp_tool(
            data["run"].as_str().unwrap(),
            data["token"].as_str().unwrap(),
            &tool,
            &mut session,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    panic!("Child must be killed before a remote response");
}

#[tokio::test]
async fn hard_crash_after_remote_effect_retains_the_dispatch_marker() {
    use cowork_domain::{canonical_hash, hash};
    use cowork_storage::McpResolution;
    use serde_json::json;
    use std::{
        process::{Command, Stdio},
        sync::atomic::Ordering,
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::from_secs(30), false).await;
    let f = setup(&store, &remote, AccessMode::Autonomous).await;
    std::fs::write(
        dir.path().join("fixture.json"),
        serde_json::to_vec(
            &json!({"run":f.run.id,"token":f.token,"tool":f.tool,"endpoint":remote.endpoint}),
        )
        .unwrap(),
    )
    .unwrap();
    store.close().await;
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "mcp_crash_fixture", "--ignored", "--nocapture"])
        .env("COWORK_MCP_CRASH", dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = Child(command.spawn().unwrap());
    let start = tokio::time::Instant::now();
    while remote.state.durable_effects.load(Ordering::SeqCst) == 0 {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "Child exited before its remote effect"
        );
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "Remote effect timed out"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let store = Store::sqlite(&path).await.unwrap();
    expire(&store, &f).await;
    let detail = store.detail(&f.run.id).await.unwrap();
    assert_eq!(detail.run.state, RunState::Interrupted);
    let attempt = detail.mcp_attempts[0].clone();
    assert_eq!(attempt.state, McpAttemptState::DispatchCommitted);
    assert_eq!(
        remote.state.calls.lock().unwrap()[0]["id"],
        attempt.rpc_request_id
    );
    assert_eq!(
        store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .checkpoint
            .in_flight,
        Some(f.tool.clone())
    );
    assert!(store.resume(&f.run.id).await.is_err());
    // The controlled fixture guarantees exactly one durable append and no later work.
    store
        .reconcile_mcp(
            &f.run.id,
            &f.tool.id,
            1,
            &attempt.request_sha256,
            McpResolution::Completed {
                result: result(),
                evidence_sha256: hash(&std::fs::read(&remote.state.path).unwrap()),
            },
        )
        .await
        .unwrap();
    store.resume(&f.run.id).await.unwrap();
    let token = store.claim().await.unwrap().unwrap().lease.unwrap().token;
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    let mut new_session = remote.session().await;
    let replay = runtime
        .dispatch_mcp_tool(
            &f.run.id,
            &token,
            &f.tool,
            &mut new_session,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(
        replay.state,
        McpAttemptState::ReconciledCompleted { .. }
    ));
    assert_eq!(remote.calls(), 1);
    assert_eq!(std::fs::read(&remote.state.path).unwrap(), b"effect\n");
    let archive = store.export_order(&f.run.spec.order_id).await.unwrap();
    assert_eq!(archive.sha256, canonical_hash(&archive.data));
    assert_eq!(archive.data.runs[0].mcp_attempts, vec![replay]);
    store.close().await;
}
