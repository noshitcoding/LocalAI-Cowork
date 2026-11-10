//! Real native runtime boundaries using only this fixture's private SQLite data.
use super::*;
use sqlx::Row;

async fn pool(f: &Fixture) -> sqlx::SqlitePool {
    sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&f.path))
        .await
        .unwrap()
}

// Change both projections deliberately; a mismatched row would test corruption,
// not a backwards UTC observation. No system clock or other database is touched.
async fn raise_highwater(f: &Fixture) -> i64 {
    let db = pool(f).await;
    for _ in 0..16 {
        let row = sqlx::query(
            "SELECT revision,highwater_ms,payload FROM workflow_execution_clocks WHERE run_id=$1",
        )
        .bind(&f.run.id)
        .fetch_one(&db)
        .await
        .unwrap();
        let old: i64 = row.get("highwater_ms");
        let revision: i64 = row.get("revision");
        let mut payload: Value = serde_json::from_str(row.get::<&str, _>("payload")).unwrap();
        assert_eq!(payload["highwater_ms"].as_i64(), Some(old));
        let raised = old.max(cowork_storage::now()) + 60_000;
        payload["highwater_ms"] = json!(raised);
        let count = sqlx::query("UPDATE workflow_execution_clocks SET highwater_ms=$1,payload=$2 WHERE run_id=$3 AND revision=$4 AND highwater_ms=$5")
            .bind(raised).bind(payload.to_string()).bind(&f.run.id).bind(revision).bind(old)
            .execute(&db).await.unwrap().rows_affected();
        if count == 1 {
            db.close().await;
            return raised;
        }
        tokio::task::yield_now().await;
    }
    panic!("private clock fixture failed to update its exact metadata basis");
}

#[tokio::test]
async fn rollback_blocks_native_turn_before_missing_runtime_profile_or_http() {
    let f = fixture(Mode::Text, false, 60).await;
    let a = claim(&f, "a").await;
    raise_highwater(&f).await;
    let before = f.runtime.store.stored_run(&f.run.id).await.unwrap();
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    // This would fail at profile lookup if the early guard were absent.
    let without_profiles = Runtime::new(f.runtime.store.clone(), vec![], false).unwrap();
    let result = without_profiles
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            &f.lease,
            &a,
            "blocked-before-credential",
            &CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(result, Err(Error::WorkflowClockRollback)),
        "{result:?}"
    );
    assert!(f.server.requests.lock().unwrap().is_empty());
    assert_eq!(
        canonical_hash(&f.runtime.store.stored_run(&f.run.id).await.unwrap()),
        canonical_hash(&before)
    );
    assert_eq!(
        canonical_hash(
            &f.runtime
                .store
                .stored_workflow(&f.project, &f.run.id)
                .await
                .unwrap()
        ),
        canonical_hash(&graph)
    );
    assert!(
        f.runtime
            .store
            .workflow_model_turn(&f.project, &f.run.id, "blocked-before-credential")
            .await
            .unwrap()
            .is_none()
    );
    f.runtime.store.close().await;
}

#[tokio::test]
async fn early_clock_authority_rejects_another_valid_parent_before_runtime_profile_lookup() {
    let f = fixture(Mode::Text, false, 60).await;
    let binding = f.run.spec.workflow.as_ref().unwrap();
    let order = f.runtime.store.order(&f.run.spec.order_id).await.unwrap();
    let mut spec = f.run.spec.clone();
    spec.workflow = None;
    let other = f
        .runtime
        .store
        .create_workflow_run(
            &CreateWorkflowRun {
                order_id: order.id,
                base_revision: order.revision,
                idempotency_key: "other-parent-scope-check".into(),
                workflow_id: binding.workflow_id.clone(),
                version_id: binding.version_id.clone(),
                max_parallel: 2,
            },
            spec,
        )
        .await
        .unwrap();
    let lease = f
        .runtime
        .store
        .claim_workflow_run(&f.project, &other.id)
        .await
        .unwrap()
        .lease
        .unwrap()
        .token;
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project, &other.id)
        .await
        .unwrap();
    let node = match f
        .runtime
        .store
        .workflow_command(
            &f.project,
            &other.id,
            &lease,
            graph.progress.revision(),
            WorkflowCommand::Claim {
                node_id: "a".into(),
            },
        )
        .await
        .unwrap()
    {
        WorkflowCommandResult::Claimed(node) => node,
        _ => panic!("the second workflow must have its own valid claim"),
    };
    let other_before = f.runtime.store.stored_run(&other.id).await.unwrap();
    let graph_before = f
        .runtime
        .store
        .stored_workflow(&f.project, &other.id)
        .await
        .unwrap();
    let db = pool(&f).await;
    let other_payload: String = sqlx::query_scalar("SELECT payload FROM runs WHERE id=$1")
        .bind(&other.id)
        .fetch_one(&db)
        .await
        .unwrap();
    // The foreign run, lease and clock are valid. Only the requested row identity
    // differs; this must fail before the deliberately absent runtime profile.
    assert_eq!(
        sqlx::query("UPDATE runs SET payload=$1 WHERE id=$2")
            .bind(&other_payload)
            .bind(&f.run.id)
            .execute(&db)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
    let without_profiles = Runtime::new(f.runtime.store.clone(), vec![], false).unwrap();
    let result = without_profiles
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            &lease,
            &node,
            "invalid-parent-row-before-profile",
            &CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, Err(Error::Forbidden)), "{result:?}");
    assert!(f.server.requests.lock().unwrap().is_empty());
    assert_eq!(
        canonical_hash(&f.runtime.store.stored_run(&other.id).await.unwrap()),
        canonical_hash(&other_before)
    );
    assert_eq!(
        canonical_hash(
            &f.runtime
                .store
                .stored_workflow(&f.project, &other.id)
                .await
                .unwrap()
        ),
        canonical_hash(&graph_before)
    );
    let after: String = sqlx::query_scalar("SELECT payload FROM runs WHERE id=$1")
        .bind(&f.run.id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(after, other_payload);
    db.close().await;
    f.runtime.store.close().await;
}
#[tokio::test]
async fn rollback_fences_already_admitted_dispatch_without_refunding_or_pending_replay() {
    let f = fixture(Mode::Text, false, 60).await;
    let a = claim(&f, "a").await;
    let spec = f
        .runtime
        .store
        .workflow_node_spec(&f.project, &f.run.id, "a")
        .await
        .unwrap();
    let (revision, context) = f
        .runtime
        .store
        .workflow_model_context(
            &f.project,
            &f.run.id,
            &a,
            cowork_runtime::provider::tool_catalog(&spec),
        )
        .await
        .unwrap();
    let turn = match f
        .runtime
        .store
        .begin_workflow_model(
            &f.project,
            &f.run.id,
            &f.lease,
            revision,
            &a,
            "admitted-before-rollback",
            &context,
        )
        .await
        .unwrap()
    {
        cowork_storage::WorkflowModelAdmission::Dispatch(permit) => permit.into_dispatch(),
        _ => panic!("new admission must return its one-shot permit"),
    };
    raise_highwater(&f).await;
    let before = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert_eq!(before.token_budget.unconfirmed_tokens, 16_384);
    assert!(matches!(
        f.runtime
            .store
            .assert_workflow_model_dispatch(&f.project, &f.run.id, &f.lease, &turn,)
            .await,
        Err(Error::WorkflowClockRollback)
    ));
    assert!(matches!(
        f.runtime
            .execute_workflow_model_turn(
                &f.project,
                &f.run.id,
                &f.lease,
                &a,
                &turn.id,
                &CancellationToken::new(),
            )
            .await,
        Err(Error::UncertainEffect)
    ));
    assert!(f.server.requests.lock().unwrap().is_empty());
    let interrupted = f
        .runtime
        .store
        .interrupt_workflow_run(&f.run.id, &f.lease, "workflow_clock_rollback")
        .await
        .unwrap();
    assert_eq!(interrupted.record.state, RunState::Interrupted);
    assert!(interrupted.lease.is_none());
    let after = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert_eq!(
        canonical_hash(&after.token_budget),
        canonical_hash(&before.token_budget)
    );
    assert_eq!(after.run.steps, before.run.steps);
    assert!(after.artifacts.is_empty());
    assert!(
        f.runtime
            .store
            .workflow_model_turn(&f.project, &f.run.id, &turn.id)
            .await
            .unwrap()
            .unwrap()
            .response
            .is_none()
    );
    assert!(
        !f.runtime
            .store
            .workflow_run_view(&f.project, &f.run.id)
            .await
            .unwrap()
            .input_resume_available
    );
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.path).await.unwrap();
    let runtime = Runtime::new(reopened, vec![], false).unwrap();
    assert!(matches!(
        runtime
            .execute_workflow_model_turn(
                &f.project,
                &f.run.id,
                &f.lease,
                &a,
                &turn.id,
                &CancellationToken::new(),
            )
            .await,
        Err(Error::UncertainEffect)
    ));
    assert_eq!(
        canonical_hash(&runtime.store.detail(&f.run.id).await.unwrap().token_budget),
        canonical_hash(&before.token_budget)
    );
    assert!(f.server.requests.lock().unwrap().is_empty());
    runtime.store.close().await;
}

#[tokio::test]
async fn confirmed_turn_replay_survives_rollback_and_missing_profiles_without_new_effects() {
    let f = fixture(Mode::Text, false, 60).await;
    let a = claim(&f, "a").await;
    let confirmed = f
        .runtime
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            &f.lease,
            &a,
            "confirmed-before-rollback",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(confirmed.response.is_some());
    raise_highwater(&f).await;
    let before = f.runtime.store.stored_run(&f.run.id).await.unwrap();
    let without_profiles = Runtime::new(f.runtime.store.clone(), vec![], false).unwrap();
    let repeated = without_profiles
        .execute_workflow_model_turn(
            &f.project,
            &f.run.id,
            &f.lease,
            &a,
            &confirmed.id,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(canonical_hash(&repeated), canonical_hash(&confirmed));
    assert_eq!(
        canonical_hash(&f.runtime.store.stored_run(&f.run.id).await.unwrap()),
        canonical_hash(&before)
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 1);
    f.runtime.store.close().await;
}

#[tokio::test]
async fn native_heartbeat_clock_denial_interrupts_both_held_requests_and_retains_charges() {
    let f = queued(Mode::ClockHeld, false).await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let task = tokio::spawn(async move {
        runtime
            .run_native_workflow(&project, &run, &CancellationToken::new())
            .await
    });
    wait_requests(&f, 2).await;
    let before = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert_eq!(before.token_budget.unconfirmed_tokens, 32_768);
    raise_highwater(&f).await;
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("the monotonic heartbeat must observe the clock denial")
        .unwrap();
    assert!(result.is_err());
    let after = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert_eq!(after.run.state, RunState::Interrupted);
    assert_eq!(
        after.run.error_code.as_deref(),
        Some("workflow_clock_rollback")
    );
    assert_eq!(
        canonical_hash(&after.token_budget),
        canonical_hash(&before.token_budget)
    );
    assert_eq!(after.run.steps, before.run.steps);
    assert!(after.artifacts.is_empty());
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    assert!(
        graph
            .agents
            .values()
            .flat_map(|a| &a.turns)
            .all(|t| t.response.is_none())
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
    f.server.parallel.wait().await;
    assert!(
        !f.runtime
            .store
            .workflow_run_view(&f.project, &f.run.id)
            .await
            .unwrap()
            .input_resume_available
    );
    f.runtime.store.close().await;
}

async fn cancellation_with_clock_rollback(mode: Mode, expected: RunState) {
    use cowork_workflow_contracts::WorkflowInputAnswerIntentState;
    let f = queued(mode, true).await;
    let runtime = f.runtime.clone();
    let project = f.project.clone();
    let run = f.run.id.clone();
    let task = tokio::spawn(async move {
        runtime
            .run_native_workflow(&project, &run, &CancellationToken::new())
            .await
    });
    wait_requests(&f, 2).await;
    let question = if matches!(mode, Mode::InputParallel) {
        // The clean-cancel case must actually have two confirmed human waits,
        // not a race with a sibling response still awaiting settlement.
        two_input_questions(&f)
            .await
            .into_iter()
            .find(|q| q.node_id == "a")
            .unwrap()
    } else {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let view = f
                    .runtime
                    .store
                    .workflow_run_view(&f.project, &f.run.id)
                    .await
                    .unwrap();
                if let Some(question) = view
                    .inputs
                    .into_iter()
                    .find(|q| q.node_id == "a" && q.actionable)
                {
                    return question;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    };
    let intent = prepare_input_answer(&f, &question, "Kept exact answer\r\n\u{f6}\u{1f600}").await;
    let before = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert_eq!(
        before.token_budget.unconfirmed_tokens,
        if matches!(mode, Mode::InputParallel) {
            0
        } else {
            16_384
        }
    );
    let db = pool(&f).await;
    // Publish the higher clock after the cancellation transaction's verified
    // clock write. Its immutable staged clock remains untouched during checks;
    // the worker observes the actual cancel prefix and higher clock together.
    let highwater: i64 =
        sqlx::query_scalar("SELECT highwater_ms FROM workflow_execution_clocks WHERE run_id=$1")
            .bind(&f.run.id)
            .fetch_one(&db)
            .await
            .unwrap();
    let raised = highwater.max(cowork_storage::now()) + 60_000;
    let trigger = format!(
        "CREATE TRIGGER private_clock_rollback_on_cancel AFTER UPDATE ON workflow_execution_clocks WHEN NEW.revision>OLD.revision AND NEW.highwater_ms<{raised} AND (SELECT state FROM runs WHERE id=NEW.run_id)='cancel_requested' AND EXISTS(SELECT 1 FROM events WHERE run_id=NEW.run_id AND json_extract(payload,'$.kind')='cancel_requested') BEGIN UPDATE workflow_execution_clocks SET payload=json_set(payload,'$.highwater_ms',{raised}),highwater_ms={raised} WHERE run_id=NEW.run_id; END"
    );
    sqlx::query(&trigger).execute(&db).await.unwrap();
    assert_eq!(
        f.runtime.store.cancel(&f.run.id).await.unwrap().state,
        RunState::CancelRequested
    );
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.state, expected);
    let after = f.runtime.store.detail(&f.run.id).await.unwrap();
    assert_eq!(
        canonical_hash(&after.token_budget),
        canonical_hash(&before.token_budget)
    );
    assert_eq!(after.run.steps, before.run.steps);
    assert!(after.artifacts.is_empty());
    assert!(
        f.runtime
            .store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .lease
            .is_none()
    );
    let rejected = f
        .runtime
        .store
        .commit_workflow_input_answer_intent(&f.run.id, &intent.id)
        .await
        .unwrap();
    assert_eq!(rejected.state, WorkflowInputAnswerIntentState::Rejected);
    assert!(rejected.receipt.is_none());
    assert_eq!(
        rejected.submission.input.answer,
        intent.submission.input.answer
    );
    assert!(
        f.runtime
            .store
            .stored_workflow(&f.project, &f.run.id)
            .await
            .unwrap()
            .inputs
            .values()
            .all(|q| q.receipt.is_none())
    );
    assert!(
        !f.runtime
            .store
            .workflow_run_view(&f.project, &f.run.id)
            .await
            .unwrap()
            .input_resume_available
    );
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
    if matches!(mode, Mode::InputWithHeldB) {
        assert_eq!(
            after.run.error_code.as_deref(),
            Some("canceled_with_uncertain_effect")
        );
        assert_eq!(after.token_budget.unconfirmed_tokens, 16_384);
        f.server.parallel.wait().await;
    }
    sqlx::query("DROP TRIGGER private_clock_rollback_on_cancel")
        .execute(&db)
        .await
        .unwrap();
    db.close().await;
    f.runtime.store.close().await;
}

#[tokio::test]
async fn native_clock_rollback_still_acknowledges_confirmed_human_wait_cancellation() {
    Box::pin(cancellation_with_clock_rollback(
        Mode::InputParallel,
        RunState::Canceled,
    ))
    .await;
}

#[tokio::test]
async fn native_clock_rollback_cancellation_retains_unknown_sibling_and_saved_answer() {
    Box::pin(cancellation_with_clock_rollback(
        Mode::InputWithHeldB,
        RunState::Interrupted,
    ))
    .await;
}
