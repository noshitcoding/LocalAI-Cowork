#[path = "../../storage/tests/support/mcp.rs"]
mod support;
use cowork_contracts::*;
use cowork_domain::*;
use cowork_mcp::{Limits, SessionCipher};
use cowork_runtime::{McpSessionAccess, Runtime};
use cowork_storage::{Change, McpConnectionState, McpResolution, Store, id};
use std::{sync::atomic::Ordering, time::Duration};
use support::*;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

fn cipher() -> SessionCipher {
    SessionCipher::new("fixture", Zeroizing::new([7; 32])).unwrap()
}
fn access(cipher: &SessionCipher) -> McpSessionAccess<'_> {
    McpSessionAccess {
        cipher,
        bearer: None,
        limits: Limits {
            timeout_ms: 2000,
            ..Limits::default()
        },
    }
}
async fn next(store: &Store, f: &mut Fixture) {
    f.tool.id = id();
    store
        .mutate(&f.run.id, Some(&f.token), |r| {
            r.checkpoint.pending_tools = vec![f.tool.clone()];
            Ok(Change::default())
        })
        .await
        .unwrap();
}
#[tokio::test]
async fn approval_and_reopened_runtime_continue_one_remote_session_without_secret_export() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    let mut f = setup(&store, &remote, AccessMode::ConfirmMutations).await;
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    let cipher = cipher();
    let stop = CancellationToken::new();
    assert!(matches!(
        runtime
            .dispatch_persistent_mcp_tool(&f.run.id, &f.token, &f.tool, &access(&cipher), &stop)
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(remote.state.initializations.load(Ordering::SeqCst), 0);
    assert!(
        store
            .mcp_connection(&f.run.id, &f.server.id)
            .await
            .unwrap()
            .is_none()
    );
    approve(&store, &mut f).await;
    runtime
        .dispatch_persistent_mcp_tool(&f.run.id, &f.token, &f.tool, &access(&cipher), &stop)
        .await
        .unwrap();
    next(&store, &mut f).await;
    approve(&store, &mut f).await;
    drop(runtime);
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    let wrong = SessionCipher::new("fixture", Zeroizing::new([8; 32])).unwrap();
    assert!(matches!(
        runtime
            .dispatch_persistent_mcp_tool(&f.run.id, &f.token, &f.tool, &access(&wrong), &stop)
            .await,
        Err(Error::McpSessionUnavailable)
    ));
    assert_eq!(remote.calls(), 1);
    runtime
        .dispatch_persistent_mcp_tool(&f.run.id, &f.token, &f.tool, &access(&cipher), &stop)
        .await
        .unwrap();
    assert_eq!(remote.state.initializations.load(Ordering::SeqCst), 1);
    let used = remote.state.used_sessions.lock().unwrap().clone();
    assert_eq!(used.len(), 2);
    assert_eq!(used[0], used[1]);
    let connection = store
        .mcp_connection(&f.run.id, &f.server.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(connection.revision, 6);
    assert_eq!(connection.epoch, Some(3));
    let archive = store.export_order(&f.run.spec.order_id).await.unwrap();
    assert_eq!(archive.sha256, canonical_hash(&archive.data));
    let json = serde_json::to_string(&archive).unwrap();
    assert!(!json.contains(&used[0]));
    assert!(!json.contains("ciphertext"));
    drop(runtime);
    store.close().await;
    assert!(
        !std::fs::read(&path)
            .unwrap()
            .windows(used[0].len())
            .any(|w| w == used[0].as_bytes())
    );
}
#[tokio::test]
async fn lost_checkpoint_commit_retains_busy_state_and_never_reinitializes_after_review() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    let mut f = setup(&store, &remote, AccessMode::Autonomous).await;
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    let cipher = cipher();
    let stop = CancellationToken::new();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.to_str().unwrap()))
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_checkpoint BEFORE UPDATE ON mcp_connections WHEN NEW.revision=4 BEGIN SELECT RAISE(FAIL,'synthetic'); END").execute(&pool).await.unwrap();
    assert!(matches!(
        runtime
            .dispatch_persistent_mcp_tool(&f.run.id, &f.token, &f.tool, &access(&cipher), &stop)
            .await,
        Err(Error::UncertainEffect)
    ));
    let connection = store
        .mcp_connection(&f.run.id, &f.server.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        connection.state
            == McpConnectionState::Busy {
                tool_call_id: f.tool.id.clone()
            }
    );
    assert_eq!(store.detail(&f.run.id).await.unwrap().run.steps, 0);
    assert_eq!(remote.calls(), 1);
    sqlx::query("DROP TRIGGER reject_checkpoint")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    expire(&store, &f).await;
    let attempt = store.mcp_attempts(&f.run.id).await.unwrap().remove(0);
    store
        .reconcile_mcp(
            &f.run.id,
            &f.tool.id,
            attempt.revision,
            &attempt.request_sha256,
            McpResolution::Completed {
                result: result(),
                evidence_sha256: hash(b"effect\n"),
            },
        )
        .await
        .unwrap();
    store.resume(&f.run.id).await.unwrap();
    f.token = store.claim().await.unwrap().unwrap().lease.unwrap().token;
    let replay = runtime
        .dispatch_persistent_mcp_tool(&f.run.id, &f.token, &f.tool, &access(&cipher), &stop)
        .await
        .unwrap();
    assert!(matches!(
        replay.state,
        McpAttemptState::ReconciledCompleted { .. }
    ));
    next(&store, &mut f).await;
    assert!(matches!(
        runtime
            .dispatch_persistent_mcp_tool(&f.run.id, &f.token, &f.tool, &access(&cipher), &stop)
            .await,
        Err(Error::McpSessionUnavailable)
    ));
    assert_eq!(remote.state.initializations.load(Ordering::SeqCst), 1);
    assert_eq!(remote.calls(), 1);
}
#[tokio::test]
async fn expired_remote_session_interrupts_without_initializing_a_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("db")).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    let mut f = setup(&store, &remote, AccessMode::Autonomous).await;
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    let cipher = cipher();
    let stop = CancellationToken::new();
    runtime
        .dispatch_persistent_mcp_tool(&f.run.id, &f.token, &f.tool, &access(&cipher), &stop)
        .await
        .unwrap();
    remote.state.session_ids.lock().unwrap().clear();
    next(&store, &mut f).await;
    assert!(matches!(
        runtime
            .dispatch_persistent_mcp_tool(&f.run.id, &f.token, &f.tool, &access(&cipher), &stop)
            .await,
        Err(Error::UncertainEffect)
    ));
    assert_eq!(
        store.detail(&f.run.id).await.unwrap().run.state,
        RunState::Interrupted
    );
    assert!(matches!(
        store
            .mcp_connection(&f.run.id, &f.server.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        McpConnectionState::Unavailable { .. }
    ));
    assert_eq!(remote.state.initializations.load(Ordering::SeqCst), 1);
    assert_eq!(remote.calls(), 1);
}

#[tokio::test]
#[ignore = "Synthetic worker for persistent_session_survives_process_end and incomplete_session_never_reinitializes"]
async fn session_process_fixture() {
    use std::io::Write;
    let root = std::path::PathBuf::from(std::env::var_os("COWORK_MCP_SESSION_CRASH").unwrap());
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fixture.json")).unwrap()).unwrap();
    let store = Store::sqlite(&root.join("db")).await.unwrap();
    let runtime = Runtime::new(store, vec![], false).unwrap();
    let cipher = cipher();
    let tool: ToolCall = serde_json::from_value(value["tool"].clone()).unwrap();
    runtime
        .dispatch_persistent_mcp_tool(
            value["run"].as_str().unwrap(),
            value["token"].as_str().unwrap(),
            &tool,
            &McpSessionAccess {
                cipher: &cipher,
                bearer: None,
                limits: Limits {
                    timeout_ms: 10_000,
                    ..Limits::default()
                },
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let mut marker = std::fs::File::create(root.join("completed")).unwrap();
    marker.write_all(b"confirmed").unwrap();
    marker.sync_all().unwrap();
    tokio::time::sleep(Duration::from_secs(30)).await;
}
async fn hard_crash(stage: &str) {
    use std::process::{Command, Stdio};
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
    let remote = Remote::new(
        dir.path().join("effects"),
        if stage == "effect" {
            Duration::from_secs(30)
        } else {
            Duration::ZERO
        },
        false,
    )
    .await;
    if stage == "initialize" {
        remote
            .state
            .initialize_delay_ms
            .store(30_000, Ordering::SeqCst);
    }
    let mut f = setup(&store, &remote, AccessMode::Autonomous).await;
    std::fs::write(
        dir.path().join("fixture.json"),
        serde_json::to_vec(&serde_json::json!({"run":f.run.id,"token":f.token,"tool":f.tool}))
            .unwrap(),
    )
    .unwrap();
    store.close().await;
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "session_process_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("COWORK_MCP_SESSION_CRASH", dir.path())
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
    loop {
        let reached = match stage {
            "completed" => {
                std::fs::read(dir.path().join("completed")).is_ok_and(|b| b == b"confirmed")
            }
            "effect" => remote.state.durable_effects.load(Ordering::SeqCst) == 1,
            _ => remote.state.initializations.load(Ordering::SeqCst) == 1,
        };
        if reached {
            break;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child exited before {stage}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "timeout at {stage}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let store = Store::sqlite(&path).await.unwrap();
    expire(&store, &f).await;
    if stage == "effect" {
        assert!(store.resume(&f.run.id).await.is_err());
        let attempt = store.mcp_attempts(&f.run.id).await.unwrap().remove(0);
        store
            .reconcile_mcp(
                &f.run.id,
                &f.tool.id,
                attempt.revision,
                &attempt.request_sha256,
                McpResolution::Completed {
                    result: result(),
                    evidence_sha256: hash(b"effect\n"),
                },
            )
            .await
            .unwrap();
    }
    store.resume(&f.run.id).await.unwrap();
    f.token = store.claim().await.unwrap().unwrap().lease.unwrap().token;
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    let cipher = cipher();
    if stage != "initialize" {
        next(&store, &mut f).await;
    }
    let continuation = runtime
        .dispatch_persistent_mcp_tool(
            &f.run.id,
            &f.token,
            &f.tool,
            &access(&cipher),
            &CancellationToken::new(),
        )
        .await;
    if stage == "completed" {
        continuation.unwrap();
        assert_eq!(remote.calls(), 2);
        let sessions = remote.state.used_sessions.lock().unwrap();
        assert_eq!(sessions[0], sessions[1]);
    } else {
        assert!(matches!(continuation, Err(Error::McpSessionUnavailable)));
        assert_eq!(remote.calls(), usize::from(stage == "effect"));
    }
    assert_eq!(remote.state.initializations.load(Ordering::SeqCst), 1);
    drop(runtime);
    store.close().await;
}
#[tokio::test]
async fn persistent_session_survives_process_end_after_a_confirmed_tool() {
    hard_crash("completed").await;
}
#[tokio::test]
async fn incomplete_session_never_reinitializes_after_effect_or_initialization_loss() {
    hard_crash("effect").await;
    hard_crash("initialize").await;
}

#[tokio::test]
async fn altered_current_ciphertext_revision_or_epoch_fails_without_new_network_requests() {
    use sqlx::Row;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    let mut f = setup(&store, &remote, AccessMode::Autonomous).await;
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    let cipher = cipher();
    let stop = CancellationToken::new();
    runtime
        .dispatch_persistent_mcp_tool(&f.run.id, &f.token, &f.tool, &access(&cipher), &stop)
        .await
        .unwrap();
    next(&store, &mut f).await;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.to_str().unwrap()))
        .await
        .unwrap();
    let original: String = sqlx::query("SELECT payload FROM mcp_connections WHERE run_id=$1")
        .bind(&f.run.id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("payload");
    for field in ["epoch", "ciphertext", "checkpoint_revision"] {
        let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
        match field {
            "epoch" => value["epoch"] = serde_json::json!(99),
            "checkpoint_revision" => value["checkpoint_revision"] = serde_json::json!(2),
            _ => {
                value["sealed"]["ciphertext"][0] =
                    serde_json::json!(value["sealed"]["ciphertext"][0].as_u64().unwrap() ^ 1)
            }
        }
        sqlx::query("UPDATE mcp_connections SET payload=$1 WHERE run_id=$2")
            .bind(value.to_string())
            .bind(&f.run.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            runtime
                .dispatch_persistent_mcp_tool(&f.run.id, &f.token, &f.tool, &access(&cipher), &stop)
                .await
                .is_err(),
            "{field}"
        );
        assert_eq!(remote.calls(), 1);
        assert_eq!(remote.state.initializations.load(Ordering::SeqCst), 1);
    }
    sqlx::query("UPDATE mcp_connections SET payload=$1 WHERE run_id=$2")
        .bind(original)
        .bind(&f.run.id)
        .execute(&pool)
        .await
        .unwrap();
    runtime
        .dispatch_persistent_mcp_tool(&f.run.id, &f.token, &f.tool, &access(&cipher), &stop)
        .await
        .unwrap();
    assert_eq!(remote.calls(), 2);
    assert_eq!(remote.state.initializations.load(Ordering::SeqCst), 1);
}
