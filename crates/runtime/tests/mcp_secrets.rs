#[path = "../../storage/tests/support/mcp.rs"]
mod support;
use cowork_contracts::*;
use cowork_domain::*;
use cowork_mcp::{Endpoint, Limits, Session};
use cowork_runtime::{McpSecretAccess, Runtime};
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{Change, McpAdmission, Store, id};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use support::*;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeroize::Zeroizing;

struct Keys {
    id: KeyId,
    missing: AtomicBool,
    loads: AtomicUsize,
}
impl Keys {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            id: KeyId {
                instance: Uuid::new_v4(),
                key: Uuid::new_v4(),
            },
            missing: AtomicBool::new(false),
            loads: AtomicUsize::new(0),
        })
    }
}
impl KeyStore for Keys {
    fn load(&self, id: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        if self.missing.load(Ordering::SeqCst) || id != &self.id {
            return Err(cowork_secrets::Error::Missing);
        }
        Ok(Zeroizing::new([7; 32]))
    }
}
fn access(keys: &Arc<Keys>) -> McpSecretAccess {
    McpSecretAccess {
        keys: keys.clone(),
        active_key: keys.id,
        limits: Limits::default(),
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
async fn exact_credential_and_original_key_survive_restart_but_missing_or_revoked_secrets_block() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    *remote.state.required_bearer.lock().unwrap() = Some("synthetic-custody-only".into());
    let keys = Keys::new();
    let mut f = setup_with_credential(
        &store,
        &remote,
        AccessMode::ConfirmMutations,
        Some((keys.as_ref(), keys.id, "synthetic-custody-only")),
    )
    .await;
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    let stop = CancellationToken::new();
    let mut access = access(&keys);
    let prior = keys.loads.load(Ordering::SeqCst);
    assert!(matches!(
        runtime
            .dispatch_custodied_mcp_tool(&f.run.id, &f.token, &f.tool, &access, &stop)
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(keys.loads.load(Ordering::SeqCst), prior);
    assert_eq!(remote.state.initializations.load(Ordering::SeqCst), 0);
    approve(&store, &mut f).await;
    keys.missing.store(true, Ordering::SeqCst);
    assert!(
        runtime
            .dispatch_custodied_mcp_tool(&f.run.id, &f.token, &f.tool, &access, &stop)
            .await
            .is_err()
    );
    assert!(
        store
            .mcp_connection(&f.run.id, &f.server.id)
            .await
            .unwrap()
            .is_none()
    );
    keys.missing.store(false, Ordering::SeqCst);
    runtime
        .dispatch_custodied_mcp_tool(&f.run.id, &f.token, &f.tool, &access, &stop)
        .await
        .unwrap();
    let archive =
        serde_json::to_string(&store.export_order(&f.run.spec.order_id).await.unwrap()).unwrap();
    assert!(!archive.contains("synthetic-custody-only") && !archive.contains("ciphertext"));
    next(&store, &mut f).await;
    approve(&store, &mut f).await;
    drop(runtime);
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    access.active_key.key = Uuid::new_v4(); // New default must not replace this session's original key.
    runtime
        .dispatch_custodied_mcp_tool(&f.run.id, &f.token, &f.tool, &access, &stop)
        .await
        .unwrap();
    assert_eq!(remote.calls(), 2);
    assert_eq!(remote.state.initializations.load(Ordering::SeqCst), 1);
    next(&store, &mut f).await;
    approve(&store, &mut f).await;
    keys.missing.store(true, Ordering::SeqCst);
    assert!(
        runtime
            .dispatch_custodied_mcp_tool(&f.run.id, &f.token, &f.tool, &access, &stop)
            .await
            .is_err()
    );
    keys.missing.store(false, Ordering::SeqCst);
    store
        .revoke_mcp_credential(
            &f.server.project_id,
            f.server
                .configuration
                .credential_reference
                .as_ref()
                .unwrap(),
            1,
        )
        .await
        .unwrap();
    assert!(
        runtime
            .dispatch_custodied_mcp_tool(&f.run.id, &f.token, &f.tool, &access, &stop)
            .await
            .is_err()
    );
    assert_eq!(remote.calls(), 2);
    assert_eq!(remote.state.initializations.load(Ordering::SeqCst), 1);
    drop(runtime);
    store.close().await;
    assert!(
        !std::fs::read(path)
            .unwrap()
            .windows(b"synthetic-custody-only".len())
            .any(|w| w == b"synthetic-custody-only")
    );
}
#[tokio::test]
async fn revoked_credential_blocks_admission_and_final_dispatch_even_for_prepared_session() {
    for after_admission in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&dir.path().join("db")).await.unwrap();
        let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
        *remote.state.required_bearer.lock().unwrap() = Some("synthetic-custody-only".into());
        let keys = Keys::new();
        let f = setup_with_credential(
            &store,
            &remote,
            AccessMode::Autonomous,
            Some((keys.as_ref(), keys.id, "synthetic-custody-only")),
        )
        .await;
        let mut session = Session::connect(
            Endpoint::new(&remote.endpoint, Some("synthetic-custody-only")).unwrap(),
            Limits::default(),
        )
        .await
        .unwrap();
        session.discover().await.unwrap();
        let permit = if after_admission {
            let McpAdmission::Dispatch(permit) = store
                .admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&session, &f))
                .await
                .unwrap()
            else {
                panic!("permit");
            };
            Some(permit)
        } else {
            None
        };
        store
            .revoke_mcp_credential(
                &f.server.project_id,
                f.server
                    .configuration
                    .credential_reference
                    .as_ref()
                    .unwrap(),
                1,
            )
            .await
            .unwrap();
        if let Some(permit) = permit {
            assert!(
                store
                    .validate_mcp_dispatch(&f.run.id, &f.token, permit.attempt())
                    .await
                    .is_err()
            );
        } else {
            assert!(
                store
                    .admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&session, &f))
                    .await
                    .is_err()
            );
        }
        assert_eq!(remote.calls(), 0);
    }
}
#[tokio::test]
async fn credential_scope_tampering_cannot_initialize_or_fall_back_to_anonymous() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    let keys = Keys::new();
    let f = setup_with_credential(
        &store,
        &remote,
        AccessMode::Autonomous,
        Some((keys.as_ref(), keys.id, "synthetic-custody-only")),
    )
    .await;
    let reference = f
        .server
        .configuration
        .credential_reference
        .as_ref()
        .unwrap();
    let record = store
        .mcp_credential(&f.server.project_id, reference)
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.to_str().unwrap()))
        .await
        .unwrap();
    let runtime = Runtime::new(store.clone(), vec![], false).unwrap();
    for field in ["scope", "sealed"] {
        let mut changed = serde_json::to_value(&record).unwrap();
        if field == "scope" {
            changed[field]["server_id"] = serde_json::json!(id());
        } else {
            let byte = changed[field]["ciphertext"][0].as_u64().unwrap();
            changed[field]["ciphertext"][0] = serde_json::json!((byte + 1) % 256);
        }
        sqlx::query("UPDATE mcp_credentials SET payload=$1 WHERE reference=$2")
            .bind(changed.to_string())
            .bind(reference)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            runtime
                .dispatch_custodied_mcp_tool(
                    &f.run.id,
                    &f.token,
                    &f.tool,
                    &access(&keys),
                    &CancellationToken::new()
                )
                .await
                .is_err()
        );
        assert_eq!(remote.state.initializations.load(Ordering::SeqCst), 0);
    }
}
