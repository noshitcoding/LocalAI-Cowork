//! Actual SQLite/PostgreSQL registry transitions; no platform qualification.
use cowork_contracts::*;
use cowork_domain::{Error, execution_targets::*};
use cowork_storage::Store;
use ring::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair},
};
use sqlx::{AnyPool, Row};
use uuid::Uuid;

fn public_key() -> Vec<u8> {
    let encoded = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    Ed25519KeyPair::from_pkcs8(encoded.as_ref())
        .unwrap()
        .public_key()
        .as_ref()
        .to_vec()
}
fn identity() -> ExecutionRunnerIdentity {
    ExecutionRunnerIdentity {
        runner_id: Uuid::new_v4().to_string(),
        generation: 1,
        adapter_build_sha256: "a".repeat(64),
        control_public_key: public_key(),
        runner_public_key: public_key(),
    }
}
struct Fixture {
    store: Store,
    pool: AnyPool,
    _directory: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("targets.sqlite");
        let url = if let Ok(url) = std::env::var("COWORK_TEST_POSTGRES") {
            assert!(
                url.rsplit('/')
                    .next()
                    .is_some_and(|value| value.starts_with("cowork_execution_targets_")),
                "Use a new dedicated registry qualification database"
            );
            url
        } else {
            format!("sqlite:{}?mode=rwc", path.display())
        };
        let store = Store::connect(&url).await.unwrap();
        let pool = AnyPool::connect(&url).await.unwrap();
        Self {
            store,
            pool,
            _directory: directory,
        }
    }
    async fn history(&self, id: &str) -> Vec<String> {
        sqlx::query(
            "SELECT payload FROM execution_target_revisions WHERE target_id=$1 ORDER BY revision",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.get("payload"))
        .collect()
    }
    async fn close(self) {
        self.pool.close().await;
        self.store.close().await;
    }
}

#[tokio::test]
async fn explicit_activation_and_revocation_keep_original_revisions() {
    let f = Fixture::new().await;
    let identity = identity();
    assert!(f.store.execution_target_reference(&identity).await.is_err());
    let registered = f
        .store
        .register_execution_target(&identity, 0)
        .await
        .unwrap();
    assert_eq!(registered.state, ExecutionTargetState::Registered);
    assert!(f.store.execution_target_reference(&identity).await.is_err());
    let original = f.history(&registered.target_id).await;
    let active = f
        .store
        .set_execution_target_active(&identity, 1, true)
        .await
        .unwrap();
    let reference = target_reference(&active).unwrap();
    f.store
        .validate_execution_target(&reference, &identity)
        .await
        .unwrap();
    f.store
        .set_execution_target_active(&identity, 2, false)
        .await
        .unwrap();
    assert!(
        f.store
            .validate_execution_target(&reference, &identity)
            .await
            .is_err()
    );
    assert!(
        f.store
            .register_execution_target(&identity, 0)
            .await
            .is_err()
    );
    let reactivated = f
        .store
        .set_execution_target_active(&identity, 3, true)
        .await
        .unwrap();
    assert_eq!(reactivated.revision, 4);
    assert!(
        f.store
            .validate_execution_target(&reference, &identity)
            .await
            .is_err()
    );
    let current = f
        .store
        .register_execution_target(&identity, 4)
        .await
        .unwrap();
    assert_eq!(current, reactivated); // reading the same registration does not reactivate or advance it
    let history = f.history(&registered.target_id).await;
    assert_eq!(history.len(), 4);
    assert_eq!(history[0], original[0]);
    assert_eq!(
        f.store.execution_target(&identity).await.unwrap(),
        Some(current)
    );
    f.close().await;
}

#[tokio::test]
async fn new_generation_is_inactive_and_cannot_roll_back_or_adopt_another_controller() {
    let f = Fixture::new().await;
    let original = identity();
    f.store
        .register_execution_target(&original, 0)
        .await
        .unwrap();
    let active = f
        .store
        .set_execution_target_active(&original, 1, true)
        .await
        .unwrap();
    let reference = target_reference(&active).unwrap();
    let old_history = f.history(&active.target_id).await;
    let mut next = original.clone();
    next.generation = 2;
    next.runner_public_key = public_key();
    next.adapter_build_sha256 = "b".repeat(64);
    let registered = f.store.register_execution_target(&next, 2).await.unwrap();
    assert_eq!(registered.target_id, active.target_id);
    assert_eq!(registered.state, ExecutionTargetState::Registered);
    assert!(f.store.execution_target_reference(&next).await.is_err());
    assert!(
        f.store
            .validate_execution_target(&reference, &original)
            .await
            .is_err()
    );
    assert!(matches!(
        f.store.register_execution_target(&original, 3).await,
        Err(Error::Forbidden)
    ));
    let mut same_generation = next.clone();
    same_generation.adapter_build_sha256 = "c".repeat(64);
    assert!(
        f.store
            .register_execution_target(&same_generation, 3)
            .await
            .is_err()
    );
    let mut foreign = next.clone();
    foreign.control_public_key = public_key();
    assert_ne!(execution_target_id(&foreign).unwrap(), registered.target_id);
    assert!(
        f.store
            .set_execution_target_active(&foreign, 3, true)
            .await
            .is_err()
    );
    let history = f.history(&registered.target_id).await;
    assert_eq!(&history[..2], old_history.as_slice());
    assert_eq!(history.len(), 3);
    f.close().await;
}

#[tokio::test]
async fn concurrent_operator_changes_have_one_revision_winner() {
    let f = Fixture::new().await;
    let identity = identity();
    let registered = f
        .store
        .register_execution_target(&identity, 0)
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        f.store.set_execution_target_active(&identity, 1, true),
        f.store.set_execution_target_active(&identity, 1, false)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(Error::Conflict)) || matches!(b, Err(Error::Conflict)));
    assert_eq!(f.history(&registered.target_id).await.len(), 2);
    f.close().await;
}

#[tokio::test]
async fn invalid_bindings_and_corrupt_history_never_establish_authority() {
    let f = Fixture::new().await;
    let valid = identity();
    for mutation in 0..6 {
        let mut invalid = valid.clone();
        match mutation {
            0 => invalid.generation = 0,
            1 => invalid.generation = u64::MAX,
            2 => invalid.control_public_key = vec![0; 32],
            3 => invalid.runner_public_key = invalid.control_public_key.clone(),
            4 => invalid.runner_id = "untrusted\nrunner".into(),
            _ => invalid.adapter_build_sha256 = "invented".into(),
        }
        assert!(
            f.store
                .register_execution_target(&invalid, 0)
                .await
                .is_err()
        );
    }
    let registered = f.store.register_execution_target(&valid, 0).await.unwrap();
    let active = f
        .store
        .set_execution_target_active(&valid, 1, true)
        .await
        .unwrap();
    let reference = target_reference(&active).unwrap();
    sqlx::query(
        "UPDATE execution_target_revisions SET payload=$1 WHERE target_id=$2 AND revision=2",
    )
    .bind("{}")
    .bind(&registered.target_id)
    .execute(&f.pool)
    .await
    .unwrap();
    assert!(matches!(
        f.store.execution_target(&valid).await,
        Err(Error::Conflict)
    ));
    assert!(
        f.store
            .validate_execution_target(&reference, &valid)
            .await
            .is_err()
    );
    assert!(
        f.store
            .set_execution_target_active(&valid, 2, false)
            .await
            .is_err()
    );
    f.close().await;
}
