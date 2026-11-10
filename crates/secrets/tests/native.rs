//! Explicit tests use only fresh names and synthetic content in the real OS store.
use cowork_secrets::*;
use serde::{Deserialize, Serialize};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
struct Receipt {
    id: KeyId,
    scope: CredentialScope,
    sealed: SealedCredential,
}
struct Cleanup {
    store: NativeKeyStore,
    id: KeyId,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.store.delete(&self.id);
    }
}
#[test]
#[ignore = "Explicit isolated real OS credential-store qualification"]
fn native_key_survives_worker_termination_without_plaintext_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let receipt = dir.path().join("receipt.json");
    let instance = Uuid::new_v4();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", "native_key_child", "--nocapture"])
        .env("COWORK_TEST_KEY_INSTANCE", instance.to_string())
        .env("COWORK_TEST_KEY_RECEIPT", &receipt)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !receipt.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("key child exited before receipt: {status}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("native key receipt timeout");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let bytes = std::fs::read(&receipt).unwrap();
    let record: Receipt = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(record.id.instance, instance);
    let cleanup = Cleanup {
        store: NativeKeyStore::open(instance).unwrap(),
        id: record.id,
    };
    assert_eq!(
        open_bearer(&cleanup.store, &record.scope, &record.sealed)
            .unwrap()
            .as_str(),
        "synthetic-native-only"
    );
    assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-native-only"));
    let foreign = NativeKeyStore::open(Uuid::new_v4()).unwrap();
    assert!(matches!(foreign.load(&record.id), Err(Error::Invalid)));
    let missing = KeyId {
        instance,
        key: Uuid::new_v4(),
    };
    assert!(matches!(cleanup.store.load(&missing), Err(Error::Missing)));
    cleanup.store.delete(&record.id).unwrap();
    assert!(matches!(
        cleanup.store.load(&record.id),
        Err(Error::Missing)
    ));
    assert!(matches!(
        open_bearer(&cleanup.store, &record.scope, &record.sealed),
        Err(Error::Missing)
    ));
    // An explicitly provisioned replacement cannot open data encrypted by the old key.
    let second = cleanup.store.provision().unwrap();
    let _other = Cleanup {
        store: NativeKeyStore::open(instance).unwrap(),
        id: second,
    };
    assert_ne!(second, record.id);
    assert!(open_bearer(&cleanup.store, &record.scope, &record.sealed).is_err());
}
#[test]
#[ignore = "Child fixture invoked only by the native-store process test"]
fn native_key_child() {
    let Ok(instance) = std::env::var("COWORK_TEST_KEY_INSTANCE") else {
        return;
    };
    let path = std::path::PathBuf::from(std::env::var_os("COWORK_TEST_KEY_RECEIPT").unwrap());
    let store = NativeKeyStore::open(Uuid::parse_str(&instance).unwrap()).unwrap();
    let id = store.provision().unwrap();
    let scope = CredentialScope {
        project_id: "native-test".into(),
        server_id: "server".into(),
        reference: "bearer".into(),
        revision: 1,
        endpoint: "https://example.test/mcp".into(),
    };
    let sealed = seal_bearer(&store, id, &scope, "synthetic-native-only").unwrap();
    let bytes = serde_json::to_vec(&Receipt { id, scope, sealed }).unwrap();
    let stage = path.with_extension("stage");
    use std::io::Write;
    let mut file = std::fs::File::create(&stage).unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    std::fs::rename(stage, path).unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}
