#[path = "../../storage/tests/support/mcp.rs"]
mod mcp_support;
#[path = "../../storage/tests/support/native_keys.rs"]
mod native_keys;
use cowork_contracts::*;
use cowork_domain::Error;
use cowork_runtime::Runtime;
use cowork_storage::{Change, Store, id};
use native_keys::*;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn uncertain_native_create_or_database_commit_needs_read_only_reconcile() {
    for database_failure in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let store = Store::sqlite(&path).await.unwrap();
        let provider = Arc::new(FakeProvider::default());
        let runtime = Runtime::new(store.clone(), vec![], false)
            .unwrap()
            .with_native_key_provider(provider.clone());
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.to_str().unwrap()))
            .await
            .unwrap();
        if database_failure {
            sqlx::query("CREATE TRIGGER reject_setup BEFORE UPDATE ON native_secret_state WHEN NEW.revision=2 BEGIN SELECT RAISE(FAIL,'synthetic'); END").execute(&pool).await.unwrap();
        } else {
            provider
                .keys
                .fail_after_create
                .store(true, Ordering::SeqCst);
        }
        let setup = NativeSecretsSetup {
            idempotency_key: "first".into(),
            base_revision: 0,
        };
        assert!(runtime.setup_native_secrets(&setup).await.is_err());
        let retry = runtime.setup_native_secrets(&setup).await.unwrap();
        assert_eq!(retry.operation_state, NativeSecretsSetupState::Pending);
        assert!(!retry.store.configured);
        assert_eq!(provider.keys.creates.load(Ordering::SeqCst), 1);
        if database_failure {
            sqlx::query("DROP TRIGGER reject_setup")
                .execute(&pool)
                .await
                .unwrap();
        }
        let action = NativeSecretsResolution {
            operation_id: retry.operation_id,
            base_revision: retry.store.revision,
        };
        let finished = runtime.reconcile_native_secrets(&action).await.unwrap();
        assert!(finished.store.key_available);
        assert_eq!(provider.keys.creates.load(Ordering::SeqCst), 1);
        drop(runtime);
        pool.close().await;
        store.close().await;
        let store = Store::sqlite(&path).await.unwrap();
        let runtime = Runtime::new(store.clone(), vec![], false)
            .unwrap()
            .with_native_key_provider(provider.clone());
        assert!(runtime.native_secrets_status().await.unwrap().key_available);
        let active = store
            .native_secret_state()
            .await
            .unwrap()
            .unwrap()
            .active
            .unwrap();
        provider.corrupt(&active.key);
        let status = runtime.native_secrets_status().await.unwrap();
        assert!(status.configured && !status.key_available);
        let replay = runtime.setup_native_secrets(&setup).await.unwrap();
        assert!(!replay.store.key_available);
        assert_eq!(provider.keys.creates.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn abandoning_a_timed_out_creator_waits_for_its_fence_without_deleting_any_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    // The Store owns one SQLite connection. Observe committed reservations through
    // a separate read-only connection while the original writer fence is held.
    let observer = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .read_only(true),
        )
        .await
        .unwrap();
    let provider = Arc::new(FakeProvider::default());
    struct ReleaseCreator(Arc<FakeProvider>);
    impl Drop for ReleaseCreator {
        fn drop(&mut self) {
            self.0.keys.block_create.store(false, Ordering::SeqCst);
        }
    }
    // A failed assertion must release the blocking worker before Tokio joins it.
    let _release_creator = ReleaseCreator(provider.clone());
    provider.keys.block_create.store(true, Ordering::SeqCst);
    let runtime = Runtime::new(store.clone(), vec![], false)
        .unwrap()
        .with_native_key_provider(provider.clone());
    let worker = runtime.clone();
    let task = tokio::spawn(async move {
        worker
            .setup_native_secrets(&NativeSecretsSetup {
                idempotency_key: "first".into(),
                base_revision: 0,
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while provider.keys.creates.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let operation_id = tokio::time::timeout(
        Duration::from_secs(3),
        sqlx::query_scalar::<_, String>(
            "SELECT id FROM native_key_operations WHERE request_key=$1",
        )
        .bind("first")
        .fetch_one(&observer),
    )
    .await
    .unwrap()
    .unwrap();
    observer.close().await;
    // Exercise the unchanged real ten-second native-call deadline. Timing out
    // the caller must leave the fence with the still-running original creator.
    let creator = tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(creator, Err(Error::UncertainEffect)));
    let resolution = NativeSecretsResolution {
        operation_id: operation_id.clone(),
        base_revision: 1,
    };
    let mut abandon = Box::pin(runtime.abandon_native_secrets(&resolution));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), abandon.as_mut())
            .await
            .is_err()
    );
    assert_eq!(provider.keys.creates.load(Ordering::SeqCst), 1);
    provider.keys.block_create.store(false, Ordering::SeqCst);
    let abandoned = tokio::time::timeout(Duration::from_secs(5), abandon.as_mut())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(abandoned.operation_id, operation_id);
    assert_eq!(
        abandoned.operation_state,
        NativeSecretsSetupState::Abandoned
    );
    assert_eq!(abandoned.store.revision, 2);
    let status = runtime.native_secrets_status().await.unwrap();
    assert!(!status.configured && !status.key_available && status.pending_operation_id.is_none());
    assert!(matches!(
        runtime.reconcile_native_secrets(&resolution).await,
        Err(Error::Conflict)
    ));
    let original = store.native_key_request("first").await.unwrap().unwrap();
    assert_eq!(original.id, operation_id);
    assert_eq!(original.state, NativeSecretsSetupState::Abandoned);
    assert!(original.proof.is_none());
    let manager =
        cowork_secrets::NativeKeyProvider::open(provider.as_ref(), original.key.instance).unwrap();
    let original_proof = cowork_secrets::prove_key(manager.as_ref(), &original.key).unwrap();
    let next = runtime
        .setup_native_secrets(&NativeSecretsSetup {
            idempotency_key: "next".into(),
            base_revision: 2,
        })
        .await
        .unwrap();
    assert!(next.store.key_available);
    let active = store
        .native_secret_state()
        .await
        .unwrap()
        .unwrap()
        .active
        .unwrap();
    assert_ne!(active.key, original.key);
    cowork_secrets::verify_key(manager.as_ref(), &original_proof).unwrap();
    assert_eq!(provider.keys.creates.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn configured_dispatch_uses_persisted_identity_and_original_session_after_key_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    let provider = Arc::new(FakeProvider::default());
    let runtime = Runtime::new(store.clone(), vec![], false)
        .unwrap()
        .with_native_key_provider(provider.clone());
    assert!(!runtime.native_secrets_status().await.unwrap().configured);
    assert!(store.native_secret_state().await.unwrap().is_none());
    runtime
        .setup_native_secrets(&NativeSecretsSetup {
            idempotency_key: "first".into(),
            base_revision: 0,
        })
        .await
        .unwrap();
    let first = store
        .native_secret_state()
        .await
        .unwrap()
        .unwrap()
        .active
        .unwrap();
    let remote = mcp_support::Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    let mut f = mcp_support::setup(&store, &remote, AccessMode::Autonomous).await;
    runtime
        .dispatch_configured_mcp_tool(&f.run.id, &f.token, &f.tool, &CancellationToken::new())
        .await
        .unwrap();
    f.tool.id = id();
    store
        .mutate(&f.run.id, Some(&f.token), |r| {
            r.checkpoint.pending_tools = vec![f.tool.clone()];
            Ok(Change::default())
        })
        .await
        .unwrap();
    runtime
        .setup_native_secrets(&NativeSecretsSetup {
            idempotency_key: "rotation".into(),
            base_revision: 2,
        })
        .await
        .unwrap();
    drop(runtime);
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    let runtime = Runtime::new(store.clone(), vec![], false)
        .unwrap()
        .with_native_key_provider(provider.clone());
    let current = store
        .native_secret_state()
        .await
        .unwrap()
        .unwrap()
        .active
        .unwrap();
    assert_ne!(current.key, first.key);
    runtime
        .dispatch_configured_mcp_tool(&f.run.id, &f.token, &f.tool, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(remote.calls(), 2);
    assert_eq!(remote.state.initializations.load(Ordering::SeqCst), 1);
    provider.remove(&first.key);
    f.tool.id = id();
    store
        .mutate(&f.run.id, Some(&f.token), |r| {
            r.checkpoint.pending_tools = vec![f.tool.clone()];
            Ok(Change::default())
        })
        .await
        .unwrap();
    assert!(matches!(
        runtime
            .dispatch_configured_mcp_tool(&f.run.id, &f.token, &f.tool, &CancellationToken::new())
            .await,
        Err(Error::McpSessionUnavailable)
    ));
    assert_eq!(remote.calls(), 2);
    assert_eq!(provider.keys.creates.load(Ordering::SeqCst), 2);
}
