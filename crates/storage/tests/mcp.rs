#[path = "support/mcp.rs"]
mod support;
use cowork_contracts::*;
use cowork_domain::{mcp::*, *};
use cowork_storage::{Change, McpAdmission, McpResolution, Store};
use serde_json::json;
use std::time::Duration;
use support::*;

async fn shared_contract(store: &Store) {
    let dir = tempfile::tempdir().unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    for mode in [
        AccessMode::ReadOnly,
        AccessMode::ConfirmMutations,
        AccessMode::ConfirmAll,
        AccessMode::Autonomous,
    ] {
        let mut f = setup(store, &remote, mode.clone()).await;
        let servers = store.mcp_servers(&f.server.project_id).await.unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].configuration, f.server.configuration);
        let selected = store
            .select_mcp_tools(
                &f.server.project_id,
                &[McpToolSelection {
                    server_id: f.server.id.clone(),
                    server_revision: f.server.revision,
                    tool_name: "append_note".into(),
                }],
            )
            .await
            .unwrap();
        assert_eq!(selected, f.run.spec.mcp_tools);
        let mut session = remote.session().await;
        if mode != AccessMode::Autonomous {
            assert!(matches!(
                store
                    .admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&session, &f))
                    .await,
                Err(Error::Forbidden)
            ));
            assert!(store.mcp_attempts(&f.run.id).await.unwrap().is_empty());
            if mode == AccessMode::ReadOnly {
                store.cancel(&f.run.id).await.unwrap();
                store
                    .mutate(&f.run.id, Some(&f.token), |r| {
                        r.transition(RunState::Canceled)?;
                        r.lease = None;
                        Ok(Change::event("run_canceled", json!({})))
                    })
                    .await
                    .unwrap();
                continue;
            }
            approve(store, &mut f).await;
            let mut changed = f.tool.clone();
            changed.arguments = json!({"text":"changed after approval"});
            store
                .mutate(&f.run.id, Some(&f.token), |r| {
                    r.checkpoint.pending_tools = vec![changed.clone()];
                    Ok(Change::default())
                })
                .await
                .unwrap();
            let prepared = session
                .prepare(
                    &f.server.catalog_sha256,
                    "append_note",
                    changed.arguments.clone(),
                )
                .unwrap();
            assert!(matches!(
                store
                    .admit_mcp(&f.run.id, &f.token, &changed, prepared)
                    .await,
                Err(Error::Forbidden)
            ));
            store
                .mutate(&f.run.id, Some(&f.token), |r| {
                    r.checkpoint.pending_tools = vec![f.tool.clone()];
                    Ok(Change::default())
                })
                .await
                .unwrap();
        }
        let (a, b) = tokio::join!(
            store.admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&session, &f)),
            store.admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&session, &f))
        );
        let mut permits = vec![];
        for admission in [a.unwrap(), b.unwrap()] {
            if let McpAdmission::Dispatch(permit) = admission {
                permits.push(permit);
            }
        }
        assert_eq!(permits.len(), 1);
        let permit = permits.pop().unwrap();
        let attempt = permit.attempt().clone();
        let before = store.events(&f.run.id, 0).await.unwrap().len();
        assert!(matches!(
            store
                .admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&session, &f))
                .await
                .unwrap(),
            McpAdmission::Existing(_)
        ));
        assert_eq!(store.events(&f.run.id, 0).await.unwrap().len(), before);
        store
            .validate_mcp_dispatch(&f.run.id, &f.token, &attempt)
            .await
            .unwrap();
        let response = session.call(permit.into_prepared()).await.unwrap();
        assert_eq!(
            remote.state.calls.lock().unwrap().last().unwrap()["id"],
            attempt.rpc_request_id
        );
        let result = McpResult {
            sha256: response.response_sha256,
            response: response.response,
        };
        assert!(matches!(
            store
                .finish_mcp(&f.run.id, "stale", &f.tool.id, result.clone())
                .await,
            Err(Error::StaleLease)
        ));
        let (a, b) = tokio::join!(
            store.finish_mcp(&f.run.id, &f.token, &f.tool.id, result.clone()),
            store.finish_mcp(&f.run.id, &f.token, &f.tool.id, result)
        );
        assert_eq!(a.unwrap(), b.unwrap());
        let detail = store.detail(&f.run.id).await.unwrap();
        assert_eq!(detail.run.steps, 1);
        assert_eq!(detail.mcp_attempts.len(), 1);
        assert!(detail.uncertain_file_fingerprint.is_none());
        let archive = store.export_order(&f.run.spec.order_id).await.unwrap();
        assert_eq!(archive.format_version, 13);
        assert_eq!(archive.sha256, canonical_hash(&archive.data));
        assert_eq!(archive.data.runs[0].mcp_attempts, detail.mcp_attempts);
    }
    assert_eq!(remote.calls(), 3);
    assert_eq!(
        std::fs::read(&remote.state.path).unwrap(),
        b"effect\neffect\neffect\n"
    );
}
#[tokio::test]
async fn sqlite_all_modes_exact_admission_concurrency_receipt_and_export() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("db")).await.unwrap();
    shared_contract(&store).await;
}
#[tokio::test]
#[ignore = "Requires a dedicated PostgreSQL database in COWORK_TEST_POSTGRES"]
async fn postgres_mcp_contract() {
    let store = Store::connect(&std::env::var("COWORK_TEST_POSTGRES").unwrap())
        .await
        .unwrap();
    shared_contract(&store).await;
}
#[tokio::test]
async fn server_revisions_revocation_and_receipts_cannot_revive_old_authority() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("db")).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    let f = setup(&store, &remote, AccessMode::Autonomous).await;
    let session = remote.session().await;
    let mut config = f.server.configuration.clone();
    config.enabled = false;
    let revoked = store
        .configure_mcp_server(&f.server.project_id, &f.server.id, 1, "revoke", &config)
        .await
        .unwrap();
    assert_eq!(revoked.revision, 2);
    let replay = store
        .configure_mcp_server(
            &f.server.project_id,
            &f.server.id,
            0,
            "create",
            &f.server.configuration,
        )
        .await
        .unwrap();
    assert_eq!(replay, f.server);
    assert_eq!(
        store
            .mcp_server(&f.server.project_id, &f.server.id)
            .await
            .unwrap(),
        revoked
    );
    assert!(matches!(
        store
            .admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&session, &f))
            .await,
        Err(Error::Forbidden)
    ));
    assert!(
        store
            .configure_mcp_server(
                &f.server.project_id,
                &f.server.id,
                1,
                "revoke",
                &f.server.configuration
            )
            .await
            .is_err()
    );
    assert!(matches!(
        store.mcp_server("other-project", &f.server.id).await,
        Err(Error::NotFound)
    ));
    store.close().await;
    let reopened = Store::sqlite(&dir.path().join("db")).await.unwrap();
    assert_eq!(
        reopened
            .mcp_server(&f.server.project_id, &f.server.id)
            .await
            .unwrap(),
        revoked
    );
    assert_eq!(remote.calls(), 0);
}
#[tokio::test]
async fn uncertain_failure_retains_marker_and_requires_exact_separate_reconciliation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("db")).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, true).await;
    let f = setup(&store, &remote, AccessMode::Autonomous).await;
    let mut session = remote.session().await;
    let McpAdmission::Dispatch(permit) = store
        .admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&session, &f))
        .await
        .unwrap()
    else {
        panic!("permit")
    };
    let failure = session.call(permit.into_prepared()).await.err().unwrap();
    let uncertain = store
        .interrupt_mcp(
            &f.run.id,
            &f.token,
            &f.tool.id,
            &failure.code.to_string(),
            failure.response().cloned(),
        )
        .await
        .unwrap();
    assert_eq!(uncertain.revision, 2);
    assert!(pending(&uncertain.state));
    assert!(matches!(
        store.resume(&f.run.id).await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store.cancel(&f.run.id).await,
        Err(Error::UncertainEffect)
    ));
    assert!(matches!(
        store
            .mutate(&f.run.id, None, |r| {
                r.checkpoint.in_flight = None;
                r.checkpoint.pending_tools.clear();
                Ok(Change::default())
            })
            .await,
        Err(Error::UncertainEffect)
    ));
    assert!(
        store
            .reconcile_mcp(
                &f.run.id,
                &f.tool.id,
                1,
                &uncertain.request_sha256,
                McpResolution::NotExecuted {
                    evidence_sha256: "a".repeat(64)
                }
            )
            .await
            .is_err()
    );
    assert!(
        store
            .reconcile_mcp(
                &f.run.id,
                &f.tool.id,
                2,
                &"b".repeat(64),
                McpResolution::NotExecuted {
                    evidence_sha256: "a".repeat(64)
                }
            )
            .await
            .is_err()
    );
    // Reviewed actual output, even after current server authority was withdrawn.
    let mut config = f.server.configuration.clone();
    config.enabled = false;
    store
        .configure_mcp_server(&f.server.project_id, &f.server.id, 1, "revoke", &config)
        .await
        .unwrap();
    let evidence = hash(&std::fs::read(&remote.state.path).unwrap());
    let resolved = store
        .reconcile_mcp(
            &f.run.id,
            &f.tool.id,
            2,
            &uncertain.request_sha256,
            McpResolution::Completed {
                result: result(),
                evidence_sha256: evidence.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(resolved.revision, 3);
    for invalid_revision in [0, -1, i64::MAX] {
        assert!(matches!(
            store
                .reconcile_mcp(
                    &f.run.id,
                    &f.tool.id,
                    invalid_revision,
                    &uncertain.request_sha256,
                    McpResolution::NotExecuted {
                        evidence_sha256: "a".repeat(64)
                    }
                )
                .await,
            Err(Error::Conflict)
        ));
    }
    assert_eq!(
        store.detail(&f.run.id).await.unwrap().run.state,
        RunState::Interrupted
    );
    assert_eq!(
        store
            .reconcile_mcp(
                &f.run.id,
                &f.tool.id,
                2,
                &uncertain.request_sha256,
                McpResolution::Completed {
                    result: result(),
                    evidence_sha256: evidence
                }
            )
            .await
            .unwrap(),
        resolved
    );
    store.resume(&f.run.id).await.unwrap();
    assert_eq!(remote.calls(), 1);
    assert_eq!(std::fs::read(&remote.state.path).unwrap(), b"effect\n");
}
#[tokio::test]
async fn transaction_failure_neither_grants_dispatch_nor_erases_an_unconfirmed_effect() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    let f = setup(&store, &remote, AccessMode::Autonomous).await;
    let mut session = remote.session().await;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.to_str().unwrap()))
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_admission BEFORE INSERT ON mcp_attempts BEGIN SELECT RAISE(FAIL,'synthetic'); END").execute(&pool).await.unwrap();
    assert!(
        store
            .admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&session, &f))
            .await
            .is_err()
    );
    assert!(
        store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .checkpoint
            .in_flight
            .is_none()
    );
    assert_eq!(remote.calls(), 0);
    sqlx::query("DROP TRIGGER reject_admission")
        .execute(&pool)
        .await
        .unwrap();
    let McpAdmission::Dispatch(permit) = store
        .admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&session, &f))
        .await
        .unwrap()
    else {
        panic!("permit")
    };
    let attempt = permit.attempt().clone();
    session.call(permit.into_prepared()).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_receipt BEFORE UPDATE ON mcp_attempts BEGIN SELECT RAISE(FAIL,'synthetic'); END").execute(&pool).await.unwrap();
    assert!(
        store
            .finish_mcp(&f.run.id, &f.token, &f.tool.id, result())
            .await
            .is_err()
    );
    let stored = store.stored_run(&f.run.id).await.unwrap();
    assert_eq!(stored.checkpoint.in_flight, Some(f.tool.clone()));
    assert_eq!(stored.record.steps, 0);
    assert!(
        !store
            .events(&f.run.id, 0)
            .await
            .unwrap()
            .iter()
            .any(|e| e.kind == "mcp_completed")
    );
    sqlx::query("DROP TRIGGER reject_receipt")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    expire(&store, &f).await;
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(store.mcp_attempts(&f.run.id).await.unwrap()[0], attempt);
    store
        .reconcile_mcp(
            &f.run.id,
            &f.tool.id,
            1,
            &attempt.request_sha256,
            McpResolution::Completed {
                result: result(),
                evidence_sha256: hash(b"effect\n"),
            },
        )
        .await
        .unwrap();
    assert_eq!(remote.calls(), 1);
}

#[tokio::test]
async fn admission_rejects_other_endpoint_stale_lease_and_exhausted_budget() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("db")).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    let other = Remote::new(dir.path().join("other-effects"), Duration::ZERO, false).await;
    let f = setup(&store, &remote, AccessMode::Autonomous).await;
    let session = remote.session().await;
    let other_session = other.session().await;
    assert!(matches!(
        store
            .admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&other_session, &f))
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .admit_mcp(&f.run.id, "stale", &f.tool, prepare(&session, &f))
            .await,
        Err(Error::StaleLease)
    ));
    store
        .mutate(&f.run.id, Some(&f.token), |r| {
            r.record.steps = r.record.spec.budgets.max_steps;
            Ok(Change::default())
        })
        .await
        .unwrap();
    assert!(matches!(
        store
            .admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&session, &f))
            .await,
        Err(Error::BudgetExceeded)
    ));
    assert!(store.mcp_attempts(&f.run.id).await.unwrap().is_empty());
    assert!(
        store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .checkpoint
            .in_flight
            .is_none()
    );
    assert_eq!(remote.calls() + other.calls(), 0);
}

#[tokio::test]
async fn corrupt_rpc_identity_or_result_blocks_read_and_export_without_repair() {
    use sqlx::Row;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    let f = setup(&store, &remote, AccessMode::Autonomous).await;
    let mut session = remote.session().await;
    let McpAdmission::Dispatch(permit) = store
        .admit_mcp(&f.run.id, &f.token, &f.tool, prepare(&session, &f))
        .await
        .unwrap()
    else {
        panic!("permit")
    };
    session.call(permit.into_prepared()).await.unwrap();
    store
        .finish_mcp(&f.run.id, &f.token, &f.tool.id, result())
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.to_str().unwrap()))
        .await
        .unwrap();
    let original: String = sqlx::query("SELECT payload FROM mcp_attempts WHERE run_id=$1")
        .bind(&f.run.id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("payload");
    for field in [
        "rpc_request_id",
        "connection_id",
        "project_id",
        "id",
        "result_schema",
    ] {
        let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
        if field == "result_schema" {
            let invalid = json!({"content":[],"structuredContent":{"written":"wrong type"}});
            value["record"]["state"]["result"] =
                json!({"sha256":canonical_hash(&invalid),"response":invalid});
        } else {
            value["record"][field] = json!("changed");
        }
        let corrupted = value.to_string();
        sqlx::query("UPDATE mcp_attempts SET payload=$1 WHERE run_id=$2")
            .bind(&corrupted)
            .bind(&f.run.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(store.detail(&f.run.id).await.is_err(), "{field}");
        assert!(
            store.export_order(&f.run.spec.order_id).await.is_err(),
            "{field}"
        );
        let unchanged: String = sqlx::query("SELECT payload FROM mcp_attempts WHERE run_id=$1")
            .bind(&f.run.id)
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("payload");
        assert_eq!(unchanged, corrupted);
    }
    sqlx::query("UPDATE mcp_attempts SET payload=$1 WHERE run_id=$2")
        .bind(original)
        .bind(&f.run.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(store.detail(&f.run.id).await.unwrap().mcp_attempts.len(), 1);
    assert_eq!(remote.calls(), 1);
}

#[tokio::test]
async fn server_update_and_receipt_are_atomic_and_concurrent_edits_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::sqlite(&path).await.unwrap();
    let remote = Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    let f = setup(&store, &remote, AccessMode::Autonomous).await;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.to_str().unwrap()))
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_config_receipt BEFORE INSERT ON mcp_server_requests BEGIN SELECT RAISE(FAIL,'synthetic'); END").execute(&pool).await.unwrap();
    let mut changed = f.server.configuration.clone();
    changed.name = "Changed".into();
    assert!(
        store
            .configure_mcp_server(&f.server.project_id, &f.server.id, 1, "edit", &changed)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .mcp_server(&f.server.project_id, &f.server.id)
            .await
            .unwrap(),
        f.server
    );
    sqlx::query("DROP TRIGGER reject_config_receipt")
        .execute(&pool)
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        store.configure_mcp_server(&f.server.project_id, &f.server.id, 1, "edit-a", &changed),
        store.configure_mcp_server(&f.server.project_id, &f.server.id, 1, "edit-b", &changed)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a.err().or(b.err()), Some(Error::Conflict)));
    assert_eq!(
        store
            .mcp_server(&f.server.project_id, &f.server.id)
            .await
            .unwrap()
            .revision,
        2
    );
}
