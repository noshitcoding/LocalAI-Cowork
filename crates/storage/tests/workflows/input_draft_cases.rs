use super::input_cases::{input_fixture, opened, propose};
use super::*;
use cowork_domain::canonical_hash;
use cowork_workflow_contracts::*;
async fn question(store: &Store) -> (Fixture, RunRecord, String, WorkflowInputView) {
    let (f, run, lease) = input_fixture(store, AccessMode::ConfirmMutations).await;
    let (_, turn) = propose(
        store,
        &f,
        &run,
        &lease,
        "a",
        json!({"question":"Exact owner answer?"}),
    )
    .await;
    let q = opened(store, &f, &run, &lease, &turn).await;
    (f, run, lease, q)
}
async fn save(
    store: &Store,
    run: &RunRecord,
    q: &WorkflowInputView,
    base: i64,
    key: &str,
    text: &str,
) -> WorkflowInputDraftSave {
    store
        .save_workflow_input_draft(
            &run.id,
            &q.request_id,
            &SaveWorkflowInputDraft {
                fingerprint: q.fingerprint.clone(),
                base_revision: base,
                idempotency_key: key.into(),
                answer: text.into(),
            },
        )
        .await
        .unwrap()
}
fn saved(value: WorkflowInputDraftSave) -> (WorkflowInputDraft, WorkflowInputDraftSnapshot) {
    let WorkflowInputDraftSave::Saved { draft, snapshot } = value else {
        panic!("expected saved")
    };
    (draft, snapshot)
}
async fn prepare(
    store: &Store,
    run: &RunRecord,
    q: &WorkflowInputView,
    revision: i64,
    key: &str,
) -> WorkflowInputAnswerIntent {
    store
        .prepare_workflow_input_answer(
            &run.id,
            &q.request_id,
            &PrepareWorkflowInputAnswer {
                fingerprint: q.fingerprint.clone(),
                base_draft_revision: revision,
                idempotency_key: key.into(),
            },
        )
        .await
        .unwrap()
}
async fn draft_boundaries(store: &Store) {
    let (_, run, _, q) = question(store).await;
    let initial = store
        .workflow_input_draft(&run.id, &q.request_id)
        .await
        .unwrap();
    assert_eq!(initial.draft.revision, 0);
    assert_eq!(initial.draft.answer, "");
    assert_eq!(initial.draft.snapshot_id, None);
    assert!(initial.intents.is_empty());
    let (draft, _) = saved(save(store, &run, &q, 0, "whitespace", "  \r\n\t  ").await);
    assert_eq!(draft.answer, "  \r\n\t  ");
    assert!(matches!(
        store
            .prepare_workflow_input_answer(
                &run.id,
                &q.request_id,
                &PrepareWorkflowInputAnswer {
                    fingerprint: q.fingerprint.clone(),
                    base_draft_revision: 1,
                    idempotency_key: "invalid-empty-submit".into()
                }
            )
            .await,
        Err(Error::Validation(_))
    ));
    for (key, text) in [("nul", "A\0B".into()), ("multibyte", "😀".repeat(8001))] {
        assert!(matches!(
            store
                .save_workflow_input_draft(
                    &run.id,
                    &q.request_id,
                    &SaveWorkflowInputDraft {
                        fingerprint: q.fingerprint.clone(),
                        base_revision: 1,
                        idempotency_key: key.into(),
                        answer: text
                    }
                )
                .await,
            Err(Error::Validation(_))
        ));
    }
    assert!(matches!(
        store
            .save_workflow_input_draft(
                &run.id,
                &q.request_id,
                &SaveWorkflowInputDraft {
                    fingerprint: q.fingerprint.clone(),
                    base_revision: i64::MAX,
                    idempotency_key: "overflow".into(),
                    answer: "safe".into()
                }
            )
            .await,
        Err(Error::Validation(_))
    ));
    let lookup = store
        .workflow_input_draft(&run.id, &q.request_id)
        .await
        .unwrap();
    assert_eq!(lookup.snapshots.len(), 1);
    assert_eq!(lookup.draft.revision, 1);
}
async fn conflict_receipts(store: &Store, pool: &sqlx::AnyPool) {
    let (_, run, _, q) = question(store).await;
    let (head1, _) = saved(save(store, &run, &q, 0, "first", "first confirmed").await);
    let (head2, _) = saved(save(store, &run, &q, 1, "second", "second confirmed").await);
    let conflict = save(store, &run, &q, 1, "conflict", "incoming retained").await;
    let WorkflowInputDraftSave::Conflict { draft, snapshot } = conflict else {
        panic!("expected conflict")
    };
    assert_eq!(draft.revision, 2);
    assert_eq!(draft.answer, "second confirmed");
    assert_eq!(snapshot.base_revision, 1);
    assert_eq!(snapshot.revision, 2);
    assert_eq!(snapshot.answer, "incoming retained");
    let fetched = store
        .workflow_input_draft_snapshot(&run.id, &q.request_id, &snapshot.id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&fetched), canonical_hash(&snapshot));
    let (head3, _) = saved(save(store, &run, &q, 2, "third", "third confirmed").await);
    let replay = save(store, &run, &q, 1, "conflict", "incoming retained").await;
    assert_eq!(
        canonical_hash(&replay),
        canonical_hash(&WorkflowInputDraftSave::Conflict {
            draft: draft.clone(),
            snapshot: snapshot.clone()
        })
    );
    assert_eq!(
        store
            .workflow_input_draft(&run.id, &q.request_id)
            .await
            .unwrap()
            .draft
            .revision,
        3
    );
    assert!(matches!(
        store
            .save_workflow_input_draft(
                &run.id,
                &q.request_id,
                &SaveWorkflowInputDraft {
                    fingerprint: q.fingerprint.clone(),
                    base_revision: 1,
                    idempotency_key: "conflict".into(),
                    answer: "changed samekey".into()
                }
            )
            .await,
        Err(Error::Conflict)
    ));
    let original:String=sqlx::query_scalar("SELECT payload FROM workflow_input_draft_saves WHERE run_id=$1 AND request_id=$2 AND request_key='conflict'").bind(&run.id).bind(&q.request_id).fetch_one(pool).await.unwrap();
    let mut corrupted: serde_json::Value = serde_json::from_str(&original).unwrap();
    corrupted["head"] = json!(head3.snapshot_id);
    sqlx::query("UPDATE workflow_input_draft_saves SET payload=$1 WHERE run_id=$2 AND request_id=$3 AND request_key='conflict'").bind(corrupted.to_string()).bind(&run.id).bind(&q.request_id).execute(pool).await.unwrap();
    assert!(matches!(
        store
            .save_workflow_input_draft(
                &run.id,
                &q.request_id,
                &SaveWorkflowInputDraft {
                    fingerprint: q.fingerprint.clone(),
                    base_revision: 1,
                    idempotency_key: "conflict".into(),
                    answer: "incoming retained".into()
                }
            )
            .await,
        Err(Error::Forbidden)
    ));
    sqlx::query("UPDATE workflow_input_draft_saves SET payload=$1 WHERE run_id=$2 AND request_id=$3 AND request_key='conflict'").bind(original).bind(&run.id).bind(&q.request_id).execute(pool).await.unwrap();
    assert_eq!(head1.revision, 1);
    assert_eq!(head2.revision, 2);
}
async fn frozen_intent_and_integrity(store: &Store, pool: &sqlx::AnyPool) {
    let (f, run, lease, q) = question(store).await;
    let (_, turn_b) = propose(
        store,
        &f,
        &run,
        &lease,
        "b",
        json!({"question":"Second same raw ID"}),
    )
    .await;
    let qb = opened(store, &f, &run, &lease, &turn_b).await;
    let exact = "  ö😀\r\n\"quotes\"\\path\n  ";
    saved(save(store, &run, &q, 0, "exact-save", exact).await);
    let intent = prepare(store, &run, &q, 1, "frozen-key").await;
    let replay = prepare(store, &run, &q, 1, "frozen-key").await;
    assert_eq!(intent.id, replay.id);
    let lookup = store
        .workflow_input_draft(&run.id, &q.request_id)
        .await
        .unwrap();
    assert_eq!(lookup.intents.len(), 1);
    assert_eq!(lookup.intents[0].id, intent.id);
    saved(save(store, &run, &q, 1, "later-edit", "later editable draft").await);
    assert!(matches!(
        store
            .prepare_workflow_input_answer(
                &run.id,
                &q.request_id,
                &PrepareWorkflowInputAnswer {
                    fingerprint: q.fingerprint.clone(),
                    base_draft_revision: 2,
                    idempotency_key: "new-pending".into()
                }
            )
            .await,
        Err(Error::Conflict)
    ));
    saved(save(store, &run, &qb, 0, "b-save", "other answer").await);
    assert!(matches!(
        store
            .prepare_workflow_input_answer(
                &run.id,
                &qb.request_id,
                &PrepareWorkflowInputAnswer {
                    fingerprint: qb.fingerprint.clone(),
                    base_draft_revision: 1,
                    idempotency_key: "frozen-key".into()
                }
            )
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store
            .save_workflow_input_draft(
                &run.id,
                &qb.request_id,
                &SaveWorkflowInputDraft {
                    fingerprint: q.fingerprint.clone(),
                    base_revision: 1,
                    idempotency_key: "foreign-fp".into(),
                    answer: "not accepted".into()
                }
            )
            .await,
        Err(Error::Conflict)
    ));
    let (_, foreign, _, foreign_q) = question(store).await;
    assert!(matches!(
        store
            .workflow_input_answer_intent(&foreign.id, &intent.id)
            .await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store
            .workflow_input_draft_snapshot(
                &foreign.id,
                &foreign_q.request_id,
                intent.draft_snapshot.snapshot_id.as_str()
            )
            .await,
        Err(Error::NotFound)
    ));
    let before = store.stored_run(&run.id).await.unwrap();
    let accepted = store
        .commit_workflow_input_answer_intent(&run.id, &intent.id)
        .await
        .unwrap();
    assert_eq!(accepted.state, WorkflowInputAnswerIntentState::Accepted);
    assert_eq!(accepted.submission.input.answer, exact);
    let receipt = accepted.receipt.as_ref().unwrap();
    assert_eq!(receipt.answer_sha256, hash(exact.as_bytes()));
    let after = store.stored_run(&run.id).await.unwrap();
    assert_eq!(after.record.last_sequence, before.record.last_sequence + 1);
    assert_eq!(after.record.steps, before.record.steps);
    assert_eq!(after.lease.as_ref().unwrap().token, lease);
    let again = store
        .commit_workflow_input_answer_intent(&run.id, &intent.id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&again), canonical_hash(&accepted));
    assert_eq!(
        store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .last_sequence,
        after.record.last_sequence
    );
    let history = store.export_order(&run.spec.order_id).await.unwrap();
    assert_eq!(history.format_version, 13);
    assert!(history.data.workflow_input_answer_records.is_some());
    let event: String =
        sqlx::query_scalar("SELECT payload FROM events WHERE run_id=$1 AND sequence=$2")
            .bind(&run.id)
            .bind(receipt.confirmed_sequence)
            .fetch_one(pool)
            .await
            .unwrap();
    let mut corrupted: serde_json::Value = serde_json::from_str(&event).unwrap();
    corrupted["kind"] = json!("model_completed");
    sqlx::query("UPDATE events SET payload=$1 WHERE run_id=$2 AND sequence=$3")
        .bind(corrupted.to_string())
        .bind(&run.id)
        .bind(receipt.confirmed_sequence)
        .execute(pool)
        .await
        .unwrap();
    assert!(matches!(
        store
            .workflow_input_answer_intent(&run.id, &intent.id)
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store.export_order(&run.spec.order_id).await,
        Err(Error::Forbidden)
    ));
    sqlx::query("UPDATE events SET payload=$1 WHERE run_id=$2 AND sequence=$3")
        .bind(event)
        .bind(&run.id)
        .bind(receipt.confirmed_sequence)
        .execute(pool)
        .await
        .unwrap();
    store.cancel(&run.id).await.unwrap();
    store
        .mutate(&run.id, Some(&lease), |r| {
            r.transition(RunState::Interrupted)?;
            r.lease = None;
            Ok(Change::event("fixture_after_accepted_cancel", json!({})))
        })
        .await
        .unwrap();
    store
        .revoke_workflow(
            &f.spec.project_id,
            &f.request.workflow_id,
            3,
            "after-confirmed-revoke",
        )
        .await
        .unwrap();
    assert_eq!(
        canonical_hash(
            &store
                .commit_workflow_input_answer_intent(&run.id, &intent.id)
                .await
                .unwrap()
        ),
        canonical_hash(&accepted)
    );
}
async fn cancel_is_definitive(store: &Store) {
    let (_, run, _, q) = question(store).await;
    saved(save(store, &run, &q, 0, "cancel-save", "must not become effect").await);
    let intent = prepare(store, &run, &q, 1, "cancel-prepare").await;
    store.cancel(&run.id).await.unwrap();
    let before = store.stored_run(&run.id).await.unwrap();
    let rejected = store
        .commit_workflow_input_answer_intent(&run.id, &intent.id)
        .await
        .unwrap();
    assert_eq!(rejected.state, WorkflowInputAnswerIntentState::Rejected);
    assert_eq!(rejected.rejection.as_ref().unwrap().code, "input_closed");
    assert!(rejected.receipt.is_none());
    assert_eq!(
        store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .last_sequence,
        before.record.last_sequence
    );
    let replay = store
        .commit_workflow_input_answer_intent(&run.id, &intent.id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&replay), canonical_hash(&rejected));
    assert_eq!(
        store
            .workflow_input_draft(&run.id, &q.request_id)
            .await
            .unwrap()
            .draft
            .answer,
        "must not become effect"
    );
}
async fn cancellation_provenance_survives_recovery(store: &Store, pool: &sqlx::AnyPool) {
    let (_, run, lease, q) = question(store).await;
    let original_save = save(store, &run, &q, 0, "before-cancel", "must remain a draft").await;
    let intent = prepare(store, &run, &q, 1, "before-cancel-intent").await;
    let canceled = store.cancel(&run.id).await.unwrap();
    assert_eq!(canceled.state, RunState::CancelRequested);
    // An unknown model/worker outcome still requires conservative interruption.
    store
        .mutate(&run.id, Some(&lease), |r| {
            r.transition(RunState::Interrupted)?;
            r.lease = None;
            Ok(Change::event("fixture_cancel_recovered", json!({})))
        })
        .await
        .unwrap();
    let before = store.stored_run(&run.id).await.unwrap();
    assert_eq!(before.record.state, RunState::Interrupted);
    assert!(before.lease.is_none());
    let event = store
        .events(&run.id, 0)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "cancel_requested")
        .unwrap();
    let original_event: String =
        sqlx::query_scalar("SELECT payload FROM events WHERE run_id=$1 AND sequence=$2")
            .bind(&run.id)
            .bind(event.sequence)
            .fetch_one(pool)
            .await
            .unwrap();
    let new_save = SaveWorkflowInputDraft {
        fingerprint: q.fingerprint.clone(),
        base_revision: 1,
        idempotency_key: "no-edit-after-known-cancel".into(),
        answer: "must not create another copy".into(),
    };
    for field in ["run_id", "sequence", "id", "state"] {
        let mut corrupt: serde_json::Value = serde_json::from_str(&original_event).unwrap();
        match field {
            "run_id" => corrupt["run_id"] = json!("foreign-run"),
            "sequence" => corrupt["sequence"] = json!(before.record.last_sequence + 1),
            "id" => corrupt["id"] = json!("foreign-event"),
            "state" => corrupt["payload"]["state"] = json!("running"),
            _ => unreachable!(),
        }
        sqlx::query("UPDATE events SET payload=$1 WHERE run_id=$2 AND sequence=$3")
            .bind(corrupt.to_string())
            .bind(&run.id)
            .bind(event.sequence)
            .execute(pool)
            .await
            .unwrap();
        assert!(
            matches!(
                store
                    .commit_workflow_input_answer_intent(&run.id, &intent.id)
                    .await,
                Err(Error::Forbidden)
            ),
            "corrupt cancel {field} must not become rejection authority"
        );
        assert!(matches!(
            store
                .save_workflow_input_draft(&run.id, &q.request_id, &new_save)
                .await,
            Err(Error::Forbidden)
        ));
        assert_eq!(
            store
                .workflow_input_answer_intent(&run.id, &intent.id)
                .await
                .unwrap()
                .state,
            WorkflowInputAnswerIntentState::Pending
        );
        sqlx::query("UPDATE events SET payload=$1 WHERE run_id=$2 AND sequence=$3")
            .bind(&original_event)
            .bind(&run.id)
            .bind(event.sequence)
            .execute(pool)
            .await
            .unwrap();
    }
    assert!(matches!(
        store
            .save_workflow_input_draft(&run.id, &q.request_id, &new_save)
            .await,
        Err(Error::Forbidden)
    ));
    // Deleting cancellation evidence must never reopen an interrupted question.
    assert_eq!(
        sqlx::query("DELETE FROM events WHERE id=$1 AND run_id=$2 AND sequence=$3")
            .bind(&event.id)
            .bind(&run.id)
            .bind(event.sequence)
            .execute(pool)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
    assert!(matches!(
        store
            .commit_workflow_input_answer_intent(&run.id, &intent.id)
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .save_workflow_input_draft(&run.id, &q.request_id, &new_save)
            .await,
        Err(Error::Forbidden)
    ));
    let missing = store
        .workflow_input_draft(&run.id, &q.request_id)
        .await
        .unwrap();
    assert_eq!(missing.draft.revision, 1);
    assert_eq!(missing.snapshots.len(), 1);
    assert_eq!(
        missing.intents[0].state,
        WorkflowInputAnswerIntentState::Pending
    );
    assert_eq!(
        store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .last_sequence,
        before.record.last_sequence
    );
    sqlx::query("INSERT INTO events(id,run_id,sequence,payload) VALUES($1,$2,$3,$4)")
        .bind(&event.id)
        .bind(&run.id)
        .bind(event.sequence)
        .bind(&original_event)
        .execute(pool)
        .await
        .unwrap();
    // Lost acknowledgements still recover the already confirmed historical work.
    let replay = save(store, &run, &q, 0, "before-cancel", "must remain a draft").await;
    assert_eq!(canonical_hash(&replay), canonical_hash(&original_save));
    assert_eq!(
        prepare(store, &run, &q, 1, "before-cancel-intent").await.id,
        intent.id
    );
    let rejected = store
        .commit_workflow_input_answer_intent(&run.id, &intent.id)
        .await
        .unwrap();
    assert_eq!(rejected.state, WorkflowInputAnswerIntentState::Rejected);
    assert_eq!(rejected.rejection.as_ref().unwrap().code, "input_closed");
    assert!(rejected.receipt.is_none());
    let again = store
        .commit_workflow_input_answer_intent(&run.id, &intent.id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&again), canonical_hash(&rejected));
    let after = store.stored_run(&run.id).await.unwrap();
    assert_eq!(after.record.state, RunState::Interrupted);
    assert_eq!(after.record.last_sequence, before.record.last_sequence);
    let lookup = store
        .workflow_input_draft(&run.id, &q.request_id)
        .await
        .unwrap();
    assert_eq!(lookup.draft.revision, 1);
    assert_eq!(lookup.snapshots.len(), 1);
    assert_eq!(lookup.draft.answer, "must remain a draft");
    assert!(
        !store
            .events(&run.id, 0)
            .await
            .unwrap()
            .iter()
            .any(|e| e.kind == "workflow_input_answered")
    );
}
async fn interrupted_draft_remains_draft(store: &Store) {
    let (_, run, lease, q) = question(store).await;
    saved(save(store, &run, &q, 0, "interrupted-save", "not yet answered").await);
    let intent = prepare(store, &run, &q, 1, "interrupted-prepare").await;
    store
        .mutate(&run.id, Some(&lease), |r| {
            r.record.state = RunState::Interrupted;
            r.lease = None;
            Ok(Change::event("fixture_interrupted", json!({})))
        })
        .await
        .unwrap();
    assert!(matches!(
        store
            .commit_workflow_input_answer_intent(&run.id, &intent.id)
            .await,
        Err(Error::StaleLease)
    ));
    assert_eq!(
        store
            .workflow_input_answer_intent(&run.id, &intent.id)
            .await
            .unwrap()
            .state,
        WorkflowInputAnswerIntentState::Pending
    );
    let (draft, _) = saved(
        save(
            store,
            &run,
            &q,
            1,
            "interrupted-edit",
            "retained working text",
        )
        .await,
    );
    assert_eq!(draft.revision, 2);
    assert!(matches!(
        store
            .prepare_workflow_input_answer(
                &run.id,
                &q.request_id,
                &PrepareWorkflowInputAnswer {
                    fingerprint: q.fingerprint.clone(),
                    base_draft_revision: 2,
                    idempotency_key: "no-autoresume".into()
                }
            )
            .await,
        Err(Error::Forbidden)
    ));
}
async fn known_node_failure_is_definitive(store: &Store) {
    let (f, run, lease, q) = question(store).await;
    saved(save(store, &run, &q, 0, "nodefailure-save", "known closed node").await);
    let intent = prepare(store, &run, &q, 1, "nodefailure-prepare").await;
    let graph = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let claim = graph.agents["a"].turns.last().unwrap().claim.clone();
    store
        .workflow_command(
            &f.spec.project_id,
            &run.id,
            &lease,
            graph.progress.revision(),
            WorkflowCommand::Fail {
                claim,
                code: "known_node_failure".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.stored_run(&run.id).await.unwrap().record.state,
        RunState::Running
    );
    let rejected = store
        .commit_workflow_input_answer_intent(&run.id, &intent.id)
        .await
        .unwrap();
    assert_eq!(rejected.state, WorkflowInputAnswerIntentState::Rejected);
    assert_eq!(rejected.rejection.unwrap().code, "input_closed");
    assert!(matches!(
        store
            .save_workflow_input_draft(
                &run.id,
                &q.request_id,
                &SaveWorkflowInputDraft {
                    fingerprint: q.fingerprint.clone(),
                    base_revision: 1,
                    idempotency_key: "no-write-to-closed-node".into(),
                    answer: "blocked".into()
                }
            )
            .await,
        Err(Error::Forbidden)
    ));
}
async fn bounded_history(store: &Store) {
    let (_, run, _, q) = question(store).await;
    for revision in 0..128 {
        saved(
            save(
                store,
                &run,
                &q,
                revision,
                &format!("bounded-{revision}"),
                &format!("copy {revision}"),
            )
            .await,
        );
    }
    assert!(matches!(
        store
            .save_workflow_input_draft(
                &run.id,
                &q.request_id,
                &SaveWorkflowInputDraft {
                    fingerprint: q.fingerprint.clone(),
                    base_revision: 128,
                    idempotency_key: "quota-exceeded".into(),
                    answer: "do not falsely confirm".into()
                }
            )
            .await,
        Err(Error::BudgetExceeded)
    ));
    let lookup = store
        .workflow_input_draft(&run.id, &q.request_id)
        .await
        .unwrap();
    assert_eq!(lookup.draft.revision, 128);
    assert_eq!(lookup.snapshots.len(), 128);
}
async fn contract(store: &Store, url: &str) {
    let pool = sqlx::AnyPool::connect(url).await.unwrap();
    Box::pin(draft_boundaries(store)).await;
    Box::pin(conflict_receipts(store, &pool)).await;
    Box::pin(frozen_intent_and_integrity(store, &pool)).await;
    Box::pin(cancel_is_definitive(store)).await;
    Box::pin(cancellation_provenance_survives_recovery(store, &pool)).await;
    Box::pin(interrupted_draft_remains_draft(store)).await;
    Box::pin(known_node_failure_is_definitive(store)).await;
    Box::pin(bounded_history(store)).await;
    pool.close().await;
}
#[tokio::test]
async fn sqlite_workflow_input_draft_contract() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("answer-drafts.sqlite3");
    let url = format!("sqlite:{}?mode=rwc", path.to_string_lossy());
    let store = Store::connect(&url).await.unwrap();
    Box::pin(contract(&store, &url)).await;
    store.close().await;
}
#[tokio::test]
#[ignore = "Requires dedicated COWORK_TEST_POSTGRES database"]
async fn postgres_workflow_input_draft_contract() {
    let url = std::env::var("COWORK_TEST_POSTGRES").unwrap();
    let store = Store::connect(&url).await.unwrap();
    Box::pin(contract(&store, &url)).await;
    Box::pin(postgres_quota_wait_rechecks_question_deadline(&store, &url)).await;
    store.close().await;
}
async fn postgres_quota_wait_rechecks_question_deadline(store: &Store, url: &str) {
    let mut f = tool_cases::tool_fixture(store).await;
    f.definition.rights.tools.insert("request_input".into());
    for role in &mut f.definition.roles {
        role.rights.tools.insert("request_input".into());
    }
    for step in &mut f.definition.steps {
        step.rights.tools.insert("request_input".into());
        if step.id == "a" {
            step.budgets.max_seconds = 5;
        }
    }
    if !f.spec.allowed_tools.iter().any(|t| t == "request_input") {
        f.spec.allowed_tools.push("request_input".into());
    }
    let publication = store
        .publish_workflow(
            &f.spec.project_id,
            &f.request.workflow_id,
            2,
            "quota-deadline-input",
            &f.definition,
        )
        .await
        .unwrap();
    f.request.version_id = publication.version_id.unwrap();
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let (_, turn) = propose(
        store,
        &f,
        &run,
        &lease,
        "a",
        json!({"question":"Five second bounded metadata wait"}),
    )
    .await;
    let q = opened(store, &f, &run, &lease, &turn).await;
    saved(save(store, &run, &q, 0, "quota-initial", "last confirmed text").await);
    let before = store.stored_run(&run.id).await.unwrap();
    let pool = sqlx::PgPool::connect(url).await.unwrap();
    let mut lock = pool.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    sqlx::query("UPDATE workflow_input_quotas SET heads=heads WHERE project_id=$1")
        .bind(&f.spec.project_id)
        .execute(&mut *lock)
        .await
        .unwrap();
    let copy = store.clone();
    let run_id = run.id.clone();
    let request = q.request_id.clone();
    let input = SaveWorkflowInputDraft {
        fingerprint: q.fingerprint.clone(),
        base_revision: 1,
        idempotency_key: "quota-blocked-expired".into(),
        answer: "must roll back".into(),
    };
    let task = tokio::spawn(async move {
        copy.save_workflow_input_draft(&run_id, &request, &input)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(4),async {
        loop {
            let blocked:i64=sqlx::query_scalar("SELECT COUNT(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND (query LIKE 'UPDATE workflow_input_quotas%' OR query LIKE 'INSERT INTO workflow_input_quotas%') AND $1=ANY(pg_blocking_pids(pid))").bind(blocker).fetch_one(&pool).await.unwrap();
            if blocked>0 {break;}
            assert!(!task.is_finished(),"save must reach its own quota-row wait before deadline");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    let wait = (q.expires_at - now() + 100).max(1) as u64;
    tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
    assert!(!task.is_finished());
    lock.commit().await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(Error::Forbidden)));
    let current = store
        .workflow_input_draft(&run.id, &q.request_id)
        .await
        .unwrap();
    assert_eq!(current.draft.revision, 1);
    assert_eq!(current.draft.answer, "last confirmed text");
    assert_eq!(current.snapshots.len(), 1);
    assert_eq!(
        store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .last_sequence,
        before.record.last_sequence
    );
    pool.close().await;
}
