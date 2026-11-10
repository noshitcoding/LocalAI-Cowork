#[path = "support/native_keys.rs"]
mod native_keys;
use cowork_contracts::*;
use cowork_domain::Error;
use cowork_secrets::{NativeKeyProvider, prove_key};
use cowork_storage::*;
use native_keys::*;
use sqlx::Row;

async fn shared(store: &Store) {
    assert!(store.native_secret_state().await.unwrap().is_none());
    for revision in [-1, i64::MAX - 1, i64::MAX] {
        assert!(matches!(
            store
                .begin_native_key_setup(&NativeSecretsSetup {
                    idempotency_key: "invalid".into(),
                    base_revision: revision
                })
                .await,
            Err(Error::Conflict)
        ));
    }
    assert!(store.native_secret_state().await.unwrap().is_none());
    let input = NativeSecretsSetup {
        idempotency_key: "first".into(),
        base_revision: 0,
    };
    let (a, b) = tokio::join!(
        store.begin_native_key_setup(&input),
        store.begin_native_key_setup(&input)
    );
    let permits = [a.unwrap(), b.unwrap()]
        .into_iter()
        .filter_map(|v| {
            if let NativeKeyAdmission::Create(p) = v {
                Some(p)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(permits.len(), 1);
    let operation = permits.into_iter().next().unwrap().into_operation();
    let state = store.native_secret_state().await.unwrap().unwrap();
    assert_eq!(state.revision, 1);
    assert_eq!(state.pending_operation.as_ref(), Some(&operation.id));
    assert!(matches!(
        store
            .begin_native_key_setup(&NativeSecretsSetup {
                idempotency_key: "other".into(),
                base_revision: 1
            })
            .await,
        Err(Error::Conflict)
    ));
    let provider = FakeProvider::default();
    let manager = provider.open(operation.key.instance).unwrap();
    manager.create(&operation.key).unwrap();
    let proof = prove_key(manager.as_ref(), &operation.key).unwrap();
    let resolution = NativeSecretsResolution {
        operation_id: operation.id.clone(),
        base_revision: 1,
    };
    let mut wrong = proof.clone();
    wrong.key.key = uuid::Uuid::new_v4();
    assert!(matches!(
        store
            .finish_native_key_setup(&resolution, Some(&wrong))
            .await,
        Err(Error::Forbidden)
    ));
    let (a, b) = tokio::join!(
        store.finish_native_key_setup(&resolution, Some(&proof)),
        store.finish_native_key_setup(&resolution, Some(&proof))
    );
    assert_eq!(a.unwrap(), b.unwrap());
    let current = store.native_secret_state().await.unwrap().unwrap();
    assert_eq!(current.revision, 2);
    assert_eq!(current.active, Some(proof.clone()));
    let NativeKeyAdmission::Create(rotation) = store
        .begin_native_key_setup(&NativeSecretsSetup {
            idempotency_key: "rotation".into(),
            base_revision: 2,
        })
        .await
        .unwrap()
    else {
        panic!("rotation permit");
    };
    let rotation = rotation.into_operation();
    manager.create(&rotation.key).unwrap();
    let proof2 = prove_key(manager.as_ref(), &rotation.key).unwrap();
    let rotation_input = NativeSecretsResolution {
        operation_id: rotation.id,
        base_revision: 3,
    };
    store
        .finish_native_key_setup(&rotation_input, Some(&proof2))
        .await
        .unwrap();
    // Replaying old activation or request never restores the former selection.
    store
        .finish_native_key_setup(&resolution, Some(&proof))
        .await
        .unwrap();
    assert!(matches!(
        store.begin_native_key_setup(&input).await.unwrap(),
        NativeKeyAdmission::Existing(_)
    ));
    assert_eq!(
        store.native_secret_state().await.unwrap().unwrap().active,
        Some(proof2.clone())
    );
    let NativeKeyAdmission::Create(abandon) = store
        .begin_native_key_setup(&NativeSecretsSetup {
            idempotency_key: "abandon".into(),
            base_revision: 4,
        })
        .await
        .unwrap()
    else {
        panic!("permit");
    };
    let abandon = abandon.into_operation();
    let action = NativeSecretsResolution {
        operation_id: abandon.id.clone(),
        base_revision: 5,
    };
    let result = store.finish_native_key_setup(&action, None).await.unwrap();
    assert_eq!(result.state, NativeSecretsSetupState::Abandoned);
    assert_eq!(
        result,
        store.finish_native_key_setup(&action, None).await.unwrap()
    );
    manager.create(&abandon.key).unwrap();
    let late = prove_key(manager.as_ref(), &abandon.key).unwrap();
    assert!(matches!(
        store.finish_native_key_setup(&action, Some(&late)).await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        store.native_secret_state().await.unwrap().unwrap().active,
        Some(proof2)
    );
}
#[tokio::test]
async fn sqlite_native_setup_once_rotation_abandon_and_old_request_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    shared(&store).await;
    let state = store.native_secret_state().await.unwrap();
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(store.native_secret_state().await.unwrap(), state);
}
#[tokio::test]
async fn failed_activation_and_corrupt_native_metadata_do_not_publish_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    let NativeKeyAdmission::Create(permit) = store
        .begin_native_key_setup(&NativeSecretsSetup {
            idempotency_key: "first".into(),
            base_revision: 0,
        })
        .await
        .unwrap()
    else {
        panic!("permit");
    };
    let operation = permit.into_operation();
    let provider = FakeProvider::default();
    let manager = provider.open(operation.key.instance).unwrap();
    manager.create(&operation.key).unwrap();
    let proof = prove_key(manager.as_ref(), &operation.key).unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.to_str().unwrap()))
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_setup BEFORE UPDATE ON native_secret_state WHEN NEW.revision=2 BEGIN SELECT RAISE(FAIL,'synthetic'); END").execute(&pool).await.unwrap();
    assert!(
        store
            .finish_native_key_setup(
                &NativeSecretsResolution {
                    operation_id: operation.id.clone(),
                    base_revision: 1
                },
                Some(&proof)
            )
            .await
            .is_err()
    );
    assert_eq!(
        store
            .native_key_operation(&operation.id)
            .await
            .unwrap()
            .state,
        NativeSecretsSetupState::Pending
    );
    assert!(
        store
            .native_secret_state()
            .await
            .unwrap()
            .unwrap()
            .active
            .is_none()
    );
    sqlx::query("DROP TRIGGER reject_setup")
        .execute(&pool)
        .await
        .unwrap();
    let row = sqlx::query("SELECT payload FROM native_secret_state")
        .fetch_one(&pool)
        .await
        .unwrap();
    let original: String = row.get("payload");
    let mut changed: serde_json::Value = serde_json::from_str(&original).unwrap();
    changed["revision"] = serde_json::json!(55);
    sqlx::query("UPDATE native_secret_state SET payload=$1")
        .bind(changed.to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.native_secret_state().await.is_err());
    assert!(
        store
            .begin_native_key_setup(&NativeSecretsSetup {
                idempotency_key: "first".into(),
                base_revision: 0
            })
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "Dedicated fresh PostgreSQL database required in COWORK_TEST_POSTGRES"]
async fn postgres_native_key_setup_contract() {
    let store = Store::connect(&std::env::var("COWORK_TEST_POSTGRES").unwrap())
        .await
        .unwrap();
    shared(&store).await;
}

async fn recovery_key(
    store: &Store,
    provider: &FakeProvider,
    request: &str,
) -> cowork_secrets::KeyProof {
    let revision = store
        .native_secret_state()
        .await
        .unwrap()
        .map_or(0, |state| state.revision);
    let NativeKeyAdmission::Create(permit) = store
        .begin_native_key_setup(&NativeSecretsSetup {
            idempotency_key: request.into(),
            base_revision: revision,
        })
        .await
        .unwrap()
    else {
        panic!("new key permit")
    };
    let operation = permit.into_operation();
    let manager = provider.open(operation.key.instance).unwrap();
    manager.create(&operation.key).unwrap();
    let proof = prove_key(manager.as_ref(), &operation.key).unwrap();
    store
        .finish_native_key_setup(
            &NativeSecretsResolution {
                operation_id: operation.id,
                base_revision: revision + 1,
            },
            Some(&proof),
        )
        .await
        .unwrap();
    proof
}

#[tokio::test]
async fn sqlite_recovery_accepts_known_old_backup_and_retirement_fences_late_ciphertext() {
    use cowork_secrets::{CredentialScope, backup_keys, seal_bearer};
    use zeroize::Zeroizing;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("recovery.db"))
        .await
        .unwrap();
    let provider = FakeProvider::default();
    let old = recovery_key(&store, &provider, "recovery-first").await;
    let active = recovery_key(&store, &provider, "recovery-second").await;
    let manager = provider.open(old.key.instance).unwrap();
    let mut proofs = vec![old.clone(), active.clone()];
    proofs.sort_by_key(|p| p.key.key);
    let backup = backup_keys(
        manager.as_ref(),
        old.key.instance,
        proofs,
        Zeroizing::new("synthetic-long-password".into()),
    )
    .unwrap();
    let state = store.native_secret_state().await.unwrap().unwrap();
    // Old, known nonreferenced keys can still be restored before retirement.
    store
        .begin_native_key_restore(state.revision, &backup)
        .await
        .unwrap()
        .finish()
        .await
        .unwrap();
    assert!(matches!(
        store
            .begin_native_key_retirement(state.revision, active.key, "active-rejected")
            .await,
        Err(Error::Forbidden)
    ));
    let project = store
        .create_project(CreateProject {
            name: "recovery-fence".into(),
        })
        .await
        .unwrap();
    let scope = CredentialScope {
        project_id: project.id,
        server_id: id(),
        reference: id(),
        revision: 1,
        endpoint: "https://example.test/mcp".into(),
    };
    // Ciphertext prepared before retirement must not publish afterwards.
    let delayed = seal_bearer(manager.as_ref(), old.key, &scope, "synthetic-key").unwrap();
    let retirement = store
        .begin_native_key_retirement(state.revision, old.key, "retire-original")
        .await
        .unwrap();
    assert!(matches!(
        store.put_mcp_credential(&scope, &delayed, 0).await,
        Err(Error::NativeKeyUnavailable)
    ));
    assert!(matches!(
        store
            .begin_native_key_restore(state.revision, &backup)
            .await,
        Err(Error::NativeKeyUnavailable)
    ));
    // This is a SQL contract: physical OS deletion is covered by the manager.
    let removed = store
        .finish_native_key_retirement(&retirement)
        .await
        .unwrap();
    assert_eq!(removed.state, "removed");
    recovery_key(&store, &provider, "recovery-third").await;
    let replay = store
        .begin_native_key_retirement(state.revision, old.key, "retire-original")
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        serde_json::to_value(removed).unwrap()
    );
    store.close().await;
}

#[tokio::test]
async fn sqlite_recovery_guard_survives_timed_out_caller_until_blocking_operation_returns() {
    use cowork_secrets::backup_keys;
    use zeroize::Zeroizing;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("restore-gate.db"))
        .await
        .unwrap();
    let provider = FakeProvider::default();
    let old = recovery_key(&store, &provider, "restore-first").await;
    let active = recovery_key(&store, &provider, "restore-second").await;
    let manager = provider.open(old.key.instance).unwrap();
    let mut proofs = vec![old.clone(), active];
    proofs.sort_by_key(|p| p.key.key);
    let backup = backup_keys(
        manager.as_ref(),
        old.key.instance,
        proofs,
        Zeroizing::new("synthetic-long-password".into()),
    )
    .unwrap();
    let revision = store.native_secret_state().await.unwrap().unwrap().revision;
    let guard = store
        .begin_native_key_restore(revision, &backup)
        .await
        .unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = tokio::task::spawn_blocking(move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        drop(guard);
    });
    started_rx.await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(5), worker)
            .await
            .is_err()
    );
    let other = store.clone();
    let cleanup = tokio::spawn(async move {
        other
            .begin_native_key_retirement(revision, old.key, "after-blocking-restore")
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let waited = !cleanup.is_finished();
    release_tx.send(()).unwrap();
    assert!(
        waited,
        "retirement crossed a still-running restore after caller timeout"
    );
    let retired = tokio::time::timeout(std::time::Duration::from_secs(5), cleanup)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(retired.state, "pending");
    store.close().await;
}

#[tokio::test]
async fn sqlite_orphan_cleanup_preserves_abandoned_origin_and_blocks_late_activation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("orphan-cleanup.db"))
        .await
        .unwrap();
    let provider = FakeProvider::default();
    let NativeKeyAdmission::Create(permit) = store
        .begin_native_key_setup(&NativeSecretsSetup {
            idempotency_key: "orphan-origin".into(),
            base_revision: 0,
        })
        .await
        .unwrap()
    else {
        panic!("new candidate required")
    };
    let pending = permit.into_operation();
    let guard = store.begin_native_key_creation(&pending).await.unwrap();
    let manager = provider.open(pending.key.instance).unwrap();
    manager.create(&pending.key).unwrap();
    let observed = prove_key(manager.as_ref(), &pending.key).unwrap();
    guard.finish().await.unwrap();
    let resolution = NativeSecretsResolution {
        operation_id: pending.id.clone(),
        base_revision: pending.reserved_revision(),
    };
    let abandoned = store
        .finish_native_key_setup(&resolution, None)
        .await
        .unwrap();
    let original = serde_json::to_vec(&abandoned).unwrap();
    let revision = store.native_secret_state().await.unwrap().unwrap().revision;
    let inventory = store.native_key_cleanup_view().await.unwrap();
    assert_eq!(inventory["required_key_count"], 0);
    assert_eq!(
        inventory["candidates"][0]["orphan_operation_id"],
        pending.id
    );
    assert!(matches!(
        store.begin_native_key_creation(&pending).await,
        Err(Error::Conflict)
    ));
    let retirement = store
        .begin_native_orphan_retirement(revision, &pending.id, "original-cleanup", Some(&observed))
        .await
        .unwrap();
    assert_eq!(retirement.orphan_operation.as_ref(), Some(&abandoned));
    assert!(matches!(
        store
            .finish_native_key_setup(&resolution, Some(&observed))
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store
            .begin_native_key_retirement(revision, pending.key, "original-cleanup")
            .await,
        Err(Error::Conflict)
    ));
    let removed = store
        .finish_native_key_retirement(&retirement)
        .await
        .unwrap();
    // A lost OS reply can inspect absence yet must return the exact frozen proof.
    let replay = store
        .begin_native_orphan_retirement(revision, &pending.id, "original-cleanup", None)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&replay).unwrap(),
        serde_json::to_vec(&removed).unwrap()
    );
    assert_eq!(
        serde_json::to_vec(&store.native_key_operation(&pending.id).await.unwrap()).unwrap(),
        original
    );
    assert!(
        store
            .native_secret_state()
            .await
            .unwrap()
            .unwrap()
            .active
            .is_none()
    );
    assert_eq!(
        store.native_key_cleanup_view().await.unwrap()["remaining"],
        0
    );
    // Missing material is also bound to an actual abandoned setup, never a free key ID.
    let NativeKeyAdmission::Create(permit) = store
        .begin_native_key_setup(&NativeSecretsSetup {
            idempotency_key: "never-created".into(),
            base_revision: revision,
        })
        .await
        .unwrap()
    else {
        panic!("new candidate required")
    };
    let absent = permit.into_operation();
    store
        .finish_native_key_setup(
            &NativeSecretsResolution {
                operation_id: absent.id.clone(),
                base_revision: absent.reserved_revision(),
            },
            None,
        )
        .await
        .unwrap();
    let current = store.native_secret_state().await.unwrap().unwrap().revision;
    let missing = store
        .begin_native_orphan_retirement(current, &absent.id, "absent-cleanup", None)
        .await
        .unwrap();
    assert!(missing.proof.is_none());
    store.finish_native_key_retirement(&missing).await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn sqlite_creation_guard_fences_abandonment_after_caller_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("creation-gate.db"))
        .await
        .unwrap();
    let NativeKeyAdmission::Create(permit) = store
        .begin_native_key_setup(&NativeSecretsSetup {
            idempotency_key: "slow-original".into(),
            base_revision: 0,
        })
        .await
        .unwrap()
    else {
        panic!("new candidate required")
    };
    let original = permit.into_operation();
    let guard = store.begin_native_key_creation(&original).await.unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = tokio::task::spawn_blocking(move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        drop(guard);
    });
    started_rx.await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(5), worker)
            .await
            .is_err()
    );
    let other = store.clone();
    let completion = tokio::spawn(async move {
        other
            .finish_native_key_setup(
                &NativeSecretsResolution {
                    operation_id: original.id.clone(),
                    base_revision: original.reserved_revision(),
                },
                None,
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let held = !completion.is_finished();
    release_tx.send(()).unwrap();
    assert!(held, "abandonment passed a still-running native creation");
    let abandoned = tokio::time::timeout(std::time::Duration::from_secs(5), completion)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(abandoned.state, NativeSecretsSetupState::Abandoned);
    store.close().await;
}
