use cowork_contracts::*;
use cowork_domain::Error;
use cowork_secrets::{CredentialScope, KeyId, KeyStore, open_bearer, seal_bearer};
use cowork_storage::{Store, id};
use uuid::Uuid;
use zeroize::Zeroizing;

struct Keys;
impl KeyStore for Keys {
    fn load(&self, _: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        Ok(Zeroizing::new([7; 32]))
    }
}
async fn shared(store: &Store) {
    let project = store
        .create_project(CreateProject {
            name: "credential test".into(),
        })
        .await
        .unwrap();
    let key = KeyId {
        instance: Uuid::new_v4(),
        key: Uuid::new_v4(),
    };
    let scope = CredentialScope {
        project_id: project.id.clone(),
        server_id: id(),
        reference: id(),
        revision: 1,
        endpoint: "https://example.test/mcp".into(),
    };
    let sealed = seal_bearer(&Keys, key, &scope, "synthetic-initial").unwrap();
    let (a, b) = tokio::join!(
        store.put_mcp_credential(&scope, &sealed, 0),
        store.put_mcp_credential(&scope, &sealed, 0)
    );
    assert!(a.unwrap() == b.unwrap());
    let retrieved = store
        .mcp_credential(&project.id, &scope.reference)
        .await
        .unwrap();
    assert_eq!(
        open_bearer(&Keys, &scope, retrieved.sealed_for(&scope).unwrap())
            .unwrap()
            .as_str(),
        "synthetic-initial"
    );
    assert!(matches!(
        store.mcp_credential(&id(), &scope.reference).await,
        Err(Error::NotFound)
    ));
    let wrong = seal_bearer(&Keys, key, &scope, "synthetic-changed").unwrap();
    assert!(matches!(
        store.put_mcp_credential(&scope, &wrong, 0).await,
        Err(Error::Conflict)
    ));
    let mut rotated = scope.clone();
    rotated.revision = 2;
    let sealed2 = seal_bearer(&Keys, key, &rotated, "synthetic-rotated").unwrap();
    let mut other = rotated.clone();
    other.server_id = id();
    assert!(matches!(
        store.put_mcp_credential(&other, &sealed2, 1).await,
        Err(Error::Forbidden)
    ));
    other = rotated.clone();
    other.endpoint = "https://elsewhere.test/mcp".into();
    assert!(store.put_mcp_credential(&other, &sealed2, 1).await.is_err());
    let record = store
        .put_mcp_credential(&rotated, &sealed2, 1)
        .await
        .unwrap();
    assert!(record.sealed_for(&scope).is_err());
    assert!(matches!(
        store.put_mcp_credential(&scope, &sealed, 0).await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store
            .revoke_mcp_credential(&project.id, &scope.reference, 1)
            .await,
        Err(Error::Conflict)
    ));
    let (a, b) = tokio::join!(
        store.revoke_mcp_credential(&project.id, &scope.reference, 2),
        store.revoke_mcp_credential(&project.id, &scope.reference, 2)
    );
    assert!(a.unwrap() == b.unwrap());
    let revoked = store
        .mcp_credential(&project.id, &scope.reference)
        .await
        .unwrap();
    assert!(revoked.revoked && revoked.sealed.is_none() && revoked.revision == 3);
    assert!(revoked.sealed_for(&rotated).is_err());
    assert!(matches!(
        store.put_mcp_credential(&rotated, &sealed2, 1).await,
        Err(Error::Forbidden)
    ));
    let encoded = serde_json::to_string(&record).unwrap();
    assert!(!encoded.contains("synthetic-rotated"));
    assert!(
        store
            .revoke_mcp_credential(&project.id, &scope.reference, i64::MAX)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn sqlite_encrypted_credentials_revision_revocation_and_scope() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    shared(&store).await;
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    store.close().await;
    let data = std::fs::read(&path).unwrap();
    assert!(
        !data
            .windows(b"synthetic-".len())
            .any(|w| w == b"synthetic-")
    );
}
#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL database in COWORK_TEST_POSTGRES"]
async fn postgres_mcp_credential_contract() {
    let store = Store::connect(&std::env::var("COWORK_TEST_POSTGRES").unwrap())
        .await
        .unwrap();
    shared(&store).await;
    store.close().await;
}
