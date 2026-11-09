//! Actual OS key + SQLite process boundary. Uses fresh application/credential IDs.
use cowork_contracts::*;
use cowork_domain::Error;
use cowork_runtime::Runtime;
use cowork_secrets::{DesktopKeyProvider, NativeKeyManager, NativeKeyStore, verify_key};
use cowork_storage::Store;
use std::{
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
struct Cleanup {
    key: cowork_secrets::KeyId,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = NativeKeyStore::open(self.key.instance).and_then(|store| store.delete(&self.key));
    }
}

#[tokio::test]
#[ignore = "Explicit real native key-store and process qualification"]
async fn native_setup_process_receipt_and_uncertainty_survive_hard_kill() {
    for uncertain in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("done");
        let path = dir.path().join("db");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--ignored", "--exact", "native_setup_child", "--nocapture"])
            .env("COWORK_NATIVE_SETUP_FIXTURE", dir.path())
            .env(
                "COWORK_NATIVE_SETUP_FAILURE",
                if uncertain { "1" } else { "0" },
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !marker.exists() {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("native setup child stopped before confirmation: {status}");
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("native setup timed out");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        child.kill().unwrap();
        child.wait().unwrap();
        let store = Store::sqlite(&path).await.unwrap();
        let operation = store.native_key_request("fixture").await.unwrap().unwrap();
        let _cleanup = Cleanup { key: operation.key };
        let native = NativeKeyStore::open(operation.key.instance).unwrap();
        assert!(matches!(
            native.create(&operation.key),
            Err(cowork_secrets::Error::Conflict)
        ));
        let runtime = Runtime::new(store.clone(), vec![], false)
            .unwrap()
            .with_native_key_provider(Arc::new(DesktopKeyProvider));
        let input = NativeSecretsSetup {
            idempotency_key: "fixture".into(),
            base_revision: 0,
        };
        if uncertain {
            let repeated = runtime.setup_native_secrets(&input).await.unwrap();
            assert_eq!(repeated.operation_state, NativeSecretsSetupState::Pending);
            assert!(!repeated.store.configured);
            let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.to_str().unwrap()))
                .await
                .unwrap();
            sqlx::query("DROP TRIGGER reject_setup")
                .execute(&pool)
                .await
                .unwrap();
            pool.close().await;
            runtime
                .reconcile_native_secrets(&NativeSecretsResolution {
                    operation_id: operation.id,
                    base_revision: 1,
                })
                .await
                .unwrap();
        }
        let proof = store
            .native_secret_state()
            .await
            .unwrap()
            .unwrap()
            .active
            .unwrap();
        verify_key(&native, &proof).unwrap();
        let repeated = runtime.setup_native_secrets(&input).await.unwrap();
        assert_eq!(repeated.operation_state, NativeSecretsSetupState::Activated);
        assert_eq!(repeated.store.revision, 2);
        assert!(repeated.store.key_available);
        native.delete(&proof.key).unwrap();
        let status = runtime.native_secrets_status().await.unwrap();
        assert!(status.configured && !status.key_available);
        assert!(
            !runtime
                .setup_native_secrets(&input)
                .await
                .unwrap()
                .store
                .key_available
        );
        drop(runtime);
        store.close().await;
    }
}
#[tokio::test]
#[ignore = "Native key setup child fixture, invoked by the parent only"]
async fn native_setup_child() {
    let Some(path) = std::env::var_os("COWORK_NATIVE_SETUP_FIXTURE") else {
        return;
    };
    let dir = std::path::PathBuf::from(path);
    let path = dir.join("db");
    let store = Store::sqlite(&path).await.unwrap();
    let uncertain = std::env::var("COWORK_NATIVE_SETUP_FAILURE").as_deref() == Ok("1");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.to_str().unwrap()))
        .await
        .unwrap();
    if uncertain {
        sqlx::query("CREATE TRIGGER reject_setup BEFORE UPDATE ON native_secret_state WHEN NEW.revision=2 BEGIN SELECT RAISE(FAIL,'synthetic'); END").execute(&pool).await.unwrap();
    }
    let runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_native_key_provider(Arc::new(DesktopKeyProvider));
    let result = runtime
        .setup_native_secrets(&NativeSecretsSetup {
            idempotency_key: "fixture".into(),
            base_revision: 0,
        })
        .await;
    if uncertain {
        assert!(matches!(result, Err(Error::Storage(_))));
    } else {
        assert!(result.unwrap().store.key_available);
    }
    use std::io::Write;
    let mut marker = std::fs::File::create(dir.join("done")).unwrap();
    marker.write_all(b"confirmed test phase").unwrap();
    marker.sync_all().unwrap();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
