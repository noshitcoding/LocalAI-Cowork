//! Independent continuation contracts; no fabricated effects or provider traffic.
use super::*;
use cowork_domain::canonical_hash;
use cowork_storage::WorkflowModelAdmission;
use cowork_workflow_contracts::WorkflowOperation as JournalOperation;
use cowork_workflow_contracts::*;
use std::time::Duration;

async fn waiting(store: &Store) -> (Fixture, RunRecord, String, WorkflowInputView) {
    let (f, run, lease) = Box::pin(input_cases::input_fixture(store, AccessMode::Autonomous)).await;
    let (_, turn) = Box::pin(input_cases::propose(
        store,
        &f,
        &run,
        &lease,
        "a",
        json!({"question":"Retain my original deadline"}),
    ))
    .await;
    let question = Box::pin(input_cases::opened(store, &f, &run, &lease, &turn)).await;
    (f, run, lease, question)
}

async fn prepare(store: &Store, f: &Fixture, run: &RunRecord) -> JournalOperation {
    let view = store
        .workflow_run_view(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let preview = view.input_resume_preview;
    assert!(preview.available, "{:?}", preview.blocked_reason);
    store
        .prepare_workflow_operation(
            &f.spec.project_id,
            &PrepareWorkflowOperation {
                intent_id: id(),
                draft_snapshot: None,
                request: WorkflowOperationRequest::ResumeInput {
                    request: ResumeWorkflowInput {
                        run_id: run.id.clone(),
                        base_run_revision: preview.base_run_revision,
                        base_workflow_revision: preview.base_workflow_revision,
                        expected_checkpoint_sha256: preview.expected_checkpoint_sha256,
                        expected_cursor_sha256: preview.expected_cursor_sha256.unwrap(),
                        idempotency_key: id(),
                    },
                },
            },
        )
        .await
        .unwrap()
}

async fn generation_contract(store: &Store) {
    let (f, run, lease, question) = Box::pin(waiting(store)).await;
    let original = store.stored_run(&run.id).await.unwrap();
    store
        .interrupt_workflow_run(&run.id, &lease, "controlled_stop")
        .await
        .unwrap();
    let pending = Box::pin(prepare(store, &f, &run)).await;
    let (first, duplicate) = tokio::join!(
        store.commit_workflow_input_resume_operation(&f.spec.project_id, &pending.id),
        store.commit_workflow_input_resume_operation(&f.spec.project_id, &pending.id)
    );
    let first = first.unwrap();
    assert_eq!(canonical_hash(&first), canonical_hash(&duplicate.unwrap()));
    assert_eq!(first.state, WorkflowOperationState::Accepted);
    let queued = store.stored_run(&run.id).await.unwrap();
    assert_eq!(queued.record.state, RunState::Queued);
    assert!(queued.lease.is_none());
    assert_eq!(
        canonical_hash(&queued.record.spec),
        canonical_hash(&original.record.spec)
    );
    assert_eq!(queued.record.started_at, original.record.started_at);
    assert_eq!(queued.record.steps, original.record.steps);
    assert_eq!(
        canonical_hash(&queued.record.usage),
        canonical_hash(&original.record.usage)
    );
    assert_eq!(
        canonical_hash(&queued.checkpoint),
        canonical_hash(&original.checkpoint)
    );
    let (left, right) = tokio::join!(
        store.claim_workflow_run(&f.spec.project_id, &run.id),
        store.claim_workflow_run(&f.spec.project_id, &run.id)
    );
    let (active, refused) = match (left, right) {
        (Ok(a), Err(e)) | (Err(e), Ok(a)) => (a, e),
        _ => panic!("Exactly one controller must activate generation 1"),
    };
    assert!(matches!(
        refused,
        Error::Conflict | Error::StaleLease | Error::UncertainEffect
    ));
    let new_lease = active.lease.unwrap().token;
    assert_ne!(new_lease, lease);
    assert!(
        store
            .interrupt_workflow_run(&run.id, &lease, "stale_owner")
            .await
            .is_err()
    );
    let graph = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert_eq!(graph.format_version, 4);
    let bridge = graph.progress.human_continuations().last().unwrap();
    assert_eq!(bridge.generation, 1);
    assert_eq!(bridge.id, pending.id);
    assert_eq!(bridge.cursors[0].expires_at, question.expires_at);
    assert_eq!(bridge.cursors[0].request_id, question.request_id);
    assert!(!bridge.execution_claims.is_empty());
    assert_ne!(bridge.cursors[0].model_claim, bridge.execution_claims[0]);
    let view = store
        .workflow_run_view(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert_eq!(view.inputs[0].request_id, question.request_id);
    assert_eq!(view.inputs[0].expires_at, question.expires_at);
    assert!(view.inputs[0].receipt.is_none());
    assert_eq!(
        canonical_hash(&first.receipt),
        canonical_hash(
            &store
                .workflow_operation(&f.spec.project_id, &pending.id)
                .await
                .unwrap()
                .receipt
        )
    );
    store
        .interrupt_workflow_run(&run.id, &new_lease, "second_controlled_stop")
        .await
        .unwrap();
    let second = Box::pin(prepare(store, &f, &run)).await;
    let accepted = store
        .commit_workflow_input_resume_operation(&f.spec.project_id, &second.id)
        .await
        .unwrap();
    assert!(matches!(
        accepted.receipt,
        Some(WorkflowOperationReceipt::InputResumeQueued { generation: 2, .. })
    ));
    assert_eq!(
        canonical_hash(&first),
        canonical_hash(
            &store
                .commit_workflow_input_resume_operation(&f.spec.project_id, &pending.id)
                .await
                .unwrap()
        )
    );
    let events = store.events(&run.id, 0).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "workflow_input_resume_queued")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "workflow_input_resume_activated")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "workflow_input_answered")
            .count(),
        0
    );
    assert_eq!(
        store.stored_run(&run.id).await.unwrap().record.steps,
        original.record.steps
    );
}

async fn cursor_and_unknown_contract(store: &Store) {
    let (f, run, lease, _) = Box::pin(waiting(store)).await;
    let claim = Box::pin(claim_node(store, &f, &run, &lease, "b")).await;
    let (revision, context) = store
        .workflow_model_context(&f.spec.project_id, &run.id, &claim, tool_cases::catalog())
        .await
        .unwrap();
    let turn_id = id();
    assert!(matches!(
        store
            .begin_workflow_model(
                &f.spec.project_id,
                &run.id,
                &lease,
                revision,
                &claim,
                &turn_id,
                &context
            )
            .await
            .unwrap(),
        WorkflowModelAdmission::Dispatch(_)
    ));
    store
        .interrupt_workflow_run(&run.id, &lease, "unknown_parallel_model")
        .await
        .unwrap();
    let held = store.stored_run(&run.id).await.unwrap();
    let view = store
        .workflow_run_view(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert!(!view.input_resume_preview.available);
    assert!(view.input_resume_preview.expected_cursor_sha256.is_none());
    assert!(
        store
            .workflow_model_turn(&f.spec.project_id, &run.id, &turn_id)
            .await
            .unwrap()
            .unwrap()
            .response
            .is_none()
    );
    assert!(store.resume(&run.id).await.is_err());
    assert_eq!(
        canonical_hash(&held.checkpoint),
        canonical_hash(&store.stored_run(&run.id).await.unwrap().checkpoint)
    );

    let (f, run, lease, _) = Box::pin(waiting(store)).await;
    store
        .interrupt_workflow_run(&run.id, &lease, "cursor_check")
        .await
        .unwrap();
    let original = store.stored_run(&run.id).await.unwrap();
    let good = Box::pin(prepare(store, &f, &run)).await;
    let mut request = good.request.clone();
    if let WorkflowOperationRequest::ResumeInput { request } = &mut request {
        request.expected_cursor_sha256 = "0".repeat(64);
        request.idempotency_key = id();
    }
    let forged = store
        .prepare_workflow_operation(
            &f.spec.project_id,
            &PrepareWorkflowOperation {
                intent_id: id(),
                request,
                draft_snapshot: None,
            },
        )
        .await
        .unwrap();
    match store
        .commit_workflow_input_resume_operation(&f.spec.project_id, &forged.id)
        .await
    {
        Ok(op) => assert_ne!(op.state, WorkflowOperationState::Accepted),
        Err(e) => assert!(matches!(e, Error::Conflict | Error::Forbidden)),
    }
    assert_eq!(
        canonical_hash(&original),
        canonical_hash(&store.stored_run(&run.id).await.unwrap())
    );
    let accepted = store
        .commit_workflow_input_resume_operation(&f.spec.project_id, &good.id)
        .await
        .unwrap();
    assert_eq!(accepted.state, WorkflowOperationState::Accepted);
}

// Negative evidence-corruption fixture: ordinary API writes create every state
// and receipt first; SQL changes only the stored event version being rejected.
async fn resume_event_versions_contract() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("resume-event-versions.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let (f, run, lease, question) = Box::pin(waiting(&store)).await;
    let lookup = store
        .workflow_input_draft(&run.id, &question.request_id)
        .await
        .unwrap();
    let saved = store
        .save_workflow_input_draft(
            &run.id,
            &question.request_id,
            &SaveWorkflowInputDraft {
                fingerprint: question.fingerprint.clone(),
                base_revision: lookup.draft.revision,
                idempotency_key: id(),
                answer: "Explicit unchanged answer".into(),
            },
        )
        .await
        .unwrap();
    let WorkflowInputDraftSave::Saved { draft, .. } = saved else {
        panic!("fresh draft")
    };
    let answer = store
        .prepare_workflow_input_answer(
            &run.id,
            &question.request_id,
            &PrepareWorkflowInputAnswer {
                fingerprint: question.fingerprint.clone(),
                base_draft_revision: draft.revision,
                idempotency_key: id(),
            },
        )
        .await
        .unwrap();
    store
        .interrupt_workflow_run(&run.id, &lease, "event_version_fixture")
        .await
        .unwrap();
    let pending = Box::pin(prepare(&store, &f, &run)).await;
    let accepted = store
        .commit_workflow_input_resume_operation(&f.spec.project_id, &pending.id)
        .await
        .unwrap();
    let event = store
        .events(&run.id, 0)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "workflow_input_resume_queued")
        .unwrap();
    assert_eq!(event.schema_version, 8);
    let queued_before = store.stored_run(&run.id).await.unwrap();
    let mut corrupt = event.clone();
    corrupt.schema_version = 7;
    sqlx::query("UPDATE events SET payload=$1 WHERE id=$2")
        .bind(serde_json::to_string(&corrupt).unwrap())
        .bind(&event.id)
        .execute(&pool)
        .await
        .unwrap();
    let refused = store.claim_workflow_run(&f.spec.project_id, &run.id).await;
    assert!(
        matches!(refused, Err(Error::Forbidden | Error::Conflict)),
        "Corrupt queued schema must fail: {refused:?}"
    );
    assert!(store.stored_run(&run.id).await.unwrap().lease.is_none());
    assert_eq!(
        canonical_hash(&queued_before),
        canonical_hash(&store.stored_run(&run.id).await.unwrap())
    );
    sqlx::query("UPDATE events SET payload=$1 WHERE id=$2")
        .bind(serde_json::to_string(&event).unwrap())
        .bind(&event.id)
        .execute(&pool)
        .await
        .unwrap();
    store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let event = store
        .events(&run.id, 0)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "workflow_input_resume_activated")
        .unwrap();
    assert_eq!(event.schema_version, 8);
    let before = store.stored_run(&run.id).await.unwrap();
    let mut corrupt = event.clone();
    corrupt.schema_version = 7;
    sqlx::query("UPDATE events SET payload=$1 WHERE id=$2")
        .bind(serde_json::to_string(&corrupt).unwrap())
        .bind(&event.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store
            .commit_workflow_input_answer_intent(&run.id, &answer.id)
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        canonical_hash(&before),
        canonical_hash(&store.stored_run(&run.id).await.unwrap())
    );
    assert_eq!(
        canonical_hash(&accepted.receipt),
        canonical_hash(
            &store
                .workflow_operation(&f.spec.project_id, &pending.id)
                .await
                .unwrap()
                .receipt
        )
    );
    sqlx::query("UPDATE events SET payload=$1 WHERE id=$2")
        .bind(serde_json::to_string(&event).unwrap())
        .bind(&event.id)
        .execute(&pool)
        .await
        .unwrap();
    let active_lease = before.lease.as_ref().unwrap().token.clone();
    for field in ["id", "run_id", "sequence"] {
        let mut envelope = serde_json::to_value(&event).unwrap();
        envelope[field] = if field == "sequence" {
            json!(event.sequence + 1)
        } else {
            json!(id())
        };
        sqlx::query("UPDATE events SET payload=$1 WHERE id=$2")
            .bind(serde_json::to_string(&envelope).unwrap())
            .bind(&event.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            matches!(
                store
                    .assert_workflow_clock_authority(&f.spec.project_id, &run.id, &active_lease)
                    .await,
                Err(Error::Forbidden)
            ),
            "Corrupt activated {field} must fail before saved-turn dispatch"
        );
        assert_eq!(
            canonical_hash(&before),
            canonical_hash(&store.stored_run(&run.id).await.unwrap())
        );
    }
    sqlx::query("UPDATE events SET payload=$1 WHERE id=$2")
        .bind(serde_json::to_string(&event).unwrap())
        .bind(&event.id)
        .execute(&pool)
        .await
        .unwrap();
    store
        .assert_workflow_clock_authority(&f.spec.project_id, &run.id, &active_lease)
        .await
        .unwrap();
    assert_eq!(
        store
            .commit_workflow_input_answer_intent(&run.id, &answer.id)
            .await
            .unwrap()
            .state,
        WorkflowInputAnswerIntentState::Accepted
    );
    pool.close().await;
    store.close().await;
}

// Uses a real five-second node deadline. No timestamp or clock projection is edited.
async fn short_waiting(store: &Store) -> (Fixture, RunRecord, String, WorkflowInputView) {
    let mut f = Box::pin(tool_cases::tool_fixture(store)).await;
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
    let publication = store
        .publish_workflow(
            &f.spec.project_id,
            &f.request.workflow_id,
            2,
            &id(),
            &f.definition,
        )
        .await
        .unwrap();
    f.request.version_id = publication.version_id.unwrap();
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let lease = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap()
        .lease
        .unwrap()
        .token;
    let (_, turn) = Box::pin(input_cases::propose(
        store,
        &f,
        &run,
        &lease,
        "a",
        json!({"question":"Original five-second deadline"}),
    ))
    .await;
    let question = Box::pin(input_cases::opened(store, &f, &run, &lease, &turn)).await;
    (f, run, lease, question)
}

async fn pg_lock_deadlines(store: &Store, url: &str, activation: bool) {
    let (f, run, lease, question) = Box::pin(short_waiting(store)).await;
    store
        .interrupt_workflow_run(&run.id, &lease, "bounded_pg_lock")
        .await
        .unwrap();
    let pending = Box::pin(prepare(store, &f, &run)).await;
    let receipt = if activation {
        Some(
            store
                .commit_workflow_input_resume_operation(&f.spec.project_id, &pending.id)
                .await
                .unwrap(),
        )
    } else {
        None
    };
    let before = store.stored_run(&run.id).await.unwrap();
    let before_graph = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let pool = sqlx::PgPool::connect(url).await.unwrap();
    let before_clock: serde_json::Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>(
            "SELECT payload FROM workflow_execution_clocks WHERE run_id=$1",
        )
        .bind(&run.id)
        .fetch_one(&pool)
        .await
        .unwrap(),
    )
    .unwrap();
    let mut lock = pool.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    sqlx::query("UPDATE workflows SET revision=revision WHERE id=$1")
        .bind(&f.request.workflow_id)
        .execute(&mut *lock)
        .await
        .unwrap();
    let owned = store.clone();
    let project = f.spec.project_id.clone();
    let run_id = run.id.clone();
    let operation = pending.id.clone();
    let task = tokio::spawn(async move {
        if activation {
            owned
                .claim_workflow_run(&project, &run_id)
                .await
                .map(|_| ())
        } else {
            owned
                .commit_workflow_input_resume_operation(&project, &operation)
                .await
                .map(|o| {
                    assert_ne!(o.state, WorkflowOperationState::Accepted);
                })
        }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'UPDATE workflows%' AND $1=ANY(pg_blocking_pids(pid))").bind(blocker).fetch_one(&pool).await.unwrap();
            if count > 0 { break; }
            assert!(!task.is_finished(), "Continuation must reach its own held registry lock");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert!(
        question.expires_at > now(),
        "Deadline must be live at the observed lock wait"
    );
    tokio::time::sleep(Duration::from_millis(
        (question.expires_at - now()) as u64 + 50,
    ))
    .await;
    assert!(!task.is_finished());
    lock.commit().await.unwrap();
    let result = task.await.unwrap();
    if let Err(e) = result {
        assert!(matches!(
            e,
            Error::BudgetExceeded | Error::Conflict | Error::StaleLease
        ));
    }
    let after = store.stored_run(&run.id).await.unwrap();
    assert!(after.lease.is_none());
    if activation {
        assert_eq!(after.record.state, RunState::Interrupted);
        assert_eq!(after.record.error_code.as_deref(), Some("budget_exceeded"));
        assert_eq!(after.record.revision, before.record.revision + 1);
        assert_eq!(after.record.last_sequence, before.record.last_sequence + 1);
        let delta = store
            .events(&run.id, before.record.last_sequence)
            .await
            .unwrap();
        assert_eq!(delta.len(), 1);
        let bridge = before_graph.progress.human_continuations().last().unwrap();
        assert_eq!(delta[0].kind, "workflow_input_resume_blocked");
        assert_eq!(delta[0].schema_version, 8);
        assert_eq!(delta[0].run_id, run.id);
        assert_eq!(delta[0].sequence, before.record.last_sequence + 1);
        assert!(delta[0].created_at >= question.expires_at);
        assert_eq!(
            delta[0].payload,
            json!({"resume_id":pending.id,"generation":bridge.generation,"reason":"budget_exceeded"})
        );
        let graph = store
            .stored_workflow(&f.spec.project_id, &run.id)
            .await
            .unwrap();
        let mut expected_graph = before_graph.clone();
        expected_graph
            .progress
            .block_human(
                &expected_graph.plan,
                before_graph.progress.revision(),
                &pending.id,
                bridge.generation,
                "budget_exceeded",
                graph.progress.last_observed_at().unwrap(),
            )
            .unwrap();
        assert_eq!(canonical_hash(&graph), canonical_hash(&expected_graph));
        assert_eq!(
            canonical_hash(graph.progress.nodes()),
            canonical_hash(before_graph.progress.nodes())
        );
        assert_eq!(
            canonical_hash(graph.progress.attempts()),
            canonical_hash(before_graph.progress.attempts())
        );
        assert_eq!(
            canonical_hash(graph.progress.input_checkpoints()),
            canonical_hash(before_graph.progress.input_checkpoints())
        );
        assert_eq!(
            graph.progress.charged_tokens(),
            before_graph.progress.charged_tokens()
        );
        let blocked = graph.progress.human_continuations().last().unwrap();
        assert!(blocked.execution_claims.is_empty());
        assert!(blocked.lease_sha256.is_none());
        assert_eq!(blocked.cursors, bridge.cursors);
        assert_eq!(blocked.consumed, bridge.consumed);
        assert_eq!(blocked.blocked_reason.as_deref(), Some("budget_exceeded"));

        let after_clock: serde_json::Value = serde_json::from_str(
            &sqlx::query_scalar::<_, String>(
                "SELECT payload FROM workflow_execution_clocks WHERE run_id=$1",
            )
            .bind(&run.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        )
        .unwrap();
        let mut before_origin = before_clock.clone();
        let mut after_origin = after_clock.clone();
        for clock in [&mut before_origin, &mut after_origin] {
            for field in ["revision", "last_run_revision", "highwater_ms"] {
                clock.as_object_mut().unwrap().remove(field);
            }
        }
        assert_eq!(
            canonical_hash(&before_origin),
            canonical_hash(&after_origin)
        );
        assert_eq!(
            after_clock["revision"].as_i64().unwrap(),
            before_clock["revision"].as_i64().unwrap() + 1
        );
        assert_eq!(
            after_clock["last_run_revision"],
            json!(after.record.revision)
        );
        assert!(
            after_clock["highwater_ms"].as_i64().unwrap()
                >= before_clock["highwater_ms"].as_i64().unwrap()
        );
        println!(
            "HUMAN_RESUME_PG_LOCK_EVIDENCE {}",
            json!({"activation":true,"request_id":question.request_id,"original_expires_at":question.expires_at,"before":{"state":before.record.state,"revision":before.record.revision,"sequence":before.record.last_sequence,"started_at":before.record.started_at,"progress_revision":before_graph.progress.revision(),"clock_origin_sha256":canonical_hash(&before_origin)},"after":{"state":after.record.state,"revision":after.record.revision,"sequence":after.record.last_sequence,"started_at":after.record.started_at,"progress_revision":graph.progress.revision(),"clock_origin_sha256":canonical_hash(&after_origin),"execution_claims":blocked.execution_claims.len()},"only_new_event":delta[0]})
        );
        assert!(matches!(
            store.claim_workflow_run(&f.spec.project_id, &run.id).await,
            Err(Error::Conflict)
        ));
        assert_eq!(
            canonical_hash(&store.stored_run(&run.id).await.unwrap()),
            canonical_hash(&after)
        );
        assert_eq!(
            canonical_hash(
                &store
                    .stored_workflow(&f.spec.project_id, &run.id)
                    .await
                    .unwrap()
            ),
            canonical_hash(&graph)
        );
        assert_eq!(
            canonical_hash(
                &store
                    .events(&run.id, before.record.last_sequence)
                    .await
                    .unwrap()
            ),
            canonical_hash(&delta)
        );
        let retry_clock: serde_json::Value = serde_json::from_str(
            &sqlx::query_scalar::<_, String>(
                "SELECT payload FROM workflow_execution_clocks WHERE run_id=$1",
            )
            .bind(&run.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(canonical_hash(&retry_clock), canonical_hash(&after_clock));
    } else {
        assert_eq!(after.record.state, before.record.state);
        assert_eq!(after.record.last_sequence, before.record.last_sequence);
        assert_eq!(canonical_hash(&after), canonical_hash(&before));
        assert_eq!(
            canonical_hash(
                &store
                    .stored_workflow(&f.spec.project_id, &run.id)
                    .await
                    .unwrap()
            ),
            canonical_hash(&before_graph)
        );
        assert!(
            store
                .events(&run.id, before.record.last_sequence)
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(after.record.steps, before.record.steps);
    assert_eq!(
        canonical_hash(&after.record.spec),
        canonical_hash(&before.record.spec)
    );
    assert_eq!(after.record.started_at, before.record.started_at);
    assert_eq!(
        canonical_hash(&after.record.usage),
        canonical_hash(&before.record.usage)
    );
    assert_eq!(
        canonical_hash(&after.checkpoint),
        canonical_hash(&before.checkpoint)
    );
    if let Some(receipt) = receipt {
        assert_eq!(
            canonical_hash(&receipt.receipt),
            canonical_hash(
                &store
                    .workflow_operation(&f.spec.project_id, &pending.id)
                    .await
                    .unwrap()
                    .receipt
            )
        );
    }
    pool.close().await;
}

#[tokio::test]
async fn sqlite_workflow_input_resume_contract() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    Box::pin(generation_contract(&store)).await;
    Box::pin(cursor_and_unknown_contract(&store)).await;
    store.close().await;
    Box::pin(resume_event_versions_contract()).await;
}

#[tokio::test]
#[ignore = "Requires dedicated COWORK_TEST_POSTGRES database"]
async fn postgres_workflow_input_resume_contract() {
    let url = std::env::var("COWORK_TEST_POSTGRES").unwrap();
    let store = Store::connect(&url).await.unwrap();
    Box::pin(generation_contract(&store)).await;
    Box::pin(cursor_and_unknown_contract(&store)).await;
    Box::pin(pg_lock_deadlines(&store, &url, false)).await;
    Box::pin(pg_lock_deadlines(&store, &url, true)).await;
    store.close().await;
}
