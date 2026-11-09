//! Real draft/intent transactions with a store-local UTC source; no OS clock changes.
use super::*;
use crate::workflow_clocks::{UtcSource, WorkflowClock};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct TestUtc(Mutex<VecDeque<i64>>);
impl TestUtc {
    fn samples(&self, values: impl IntoIterator<Item = i64>) {
        *self.0.lock().unwrap() = values.into_iter().collect();
    }
}
impl UtcSource for TestUtc {
    fn now(&self) -> i64 {
        self.0.lock().unwrap().pop_front().unwrap_or_else(now)
    }
}
struct Question {
    project: String,
    run: String,
    view: WorkflowInputView,
}
async fn proposal(
    store: &Store,
    tool: &str,
    arguments: Value,
) -> (String, String, WorkflowToolExecution) {
    let project = store
        .create_project(CreateProject {
            name: "Clock input contract".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Exact answer".into(),
        })
        .await
        .unwrap();
    let provider = ProviderProfile {
        id: id(),
        name: "Explicit synthetic".into(),
        kind: ProviderKind::Test,
        endpoint: String::new(),
        model: "clock-test".into(),
        revision: 1,
        context_tokens: 8192,
        credential_required: false,
        credential_ready: true,
    };
    store.insert_provider(&provider).await.unwrap();
    let rights = WorkflowRights {
        tools: BTreeSet::from([tool.into()]),
        folders: BTreeSet::new(),
    };
    let budgets = Budgets {
        max_steps: 16,
        max_seconds: 1800,
        max_output_tokens: 256,
        max_total_tokens: 8192,
    };
    let definition = WorkflowDefinition {
        format_version: 1,
        name: "Await exact input".into(),
        mode: WorkflowMode::Sequential,
        failure: WorkflowFailure::Stop,
        rights: rights.clone(),
        budgets: budgets.clone(),
        roles: vec![WorkflowRole {
            model_binding: None,
output: None,
            id: "author".into(),
            instructions: "Ask one question".into(),
            provider_id: provider.id,
            provider_revision: 1,
            rights: rights.clone(),
            budgets: budgets.clone(),
            delegate_to: BTreeSet::new(),
        }],
        steps: vec![WorkflowStep {
            id: "a".into(),
            role_id: "author".into(),
            goal: "Get explicit answer".into(),
            dependencies: vec![],
            rights,
            budgets: budgets.clone(),
        }],
    };
    let library = store
        .create_workflow(&project.id, "create", &definition)
        .await
        .unwrap();
    let run = store
        .create_workflow_run(
            &CreateWorkflowRun {
                order_id: order.id.clone(),
                base_revision: order.revision,
                idempotency_key: "start".into(),
                workflow_id: library.id,
                version_id: library.versions[0].id.clone(),
                max_parallel: 1,
            },
            RunSpec {
            model_binding: None,
output: None,
                order_id: order.id,
                project_id: project.id.clone(),
                prompt: "Ask exact question".into(),
                provider: None,
                file_action: None,
                workflow: None,
                mcp_tools: vec![],
                environment: None,
                access_mode: AccessMode::ConfirmMutations,
                executor: "local-v2".into(),
                policy_revision: 1,
                allowed_tools: vec![tool.into()],
                budgets,
                folders: vec![],
            },
        )
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&project.id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let workflow = store.stored_workflow(&project.id, &run.id).await.unwrap();
    let WorkflowCommandResult::Claimed(claim) = store
        .workflow_command(
            &project.id,
            &run.id,
            &lease,
            workflow.progress.revision(),
            WorkflowCommand::Claim {
                node_id: "a".into(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("claim")
    };
    let (revision, context) = store
        .workflow_model_context(
            &project.id,
            &run.id,
            &claim,
            json!([{"type":"function","function":{"name":tool,"parameters":{"type":"object"}}}]),
        )
        .await
        .unwrap();
    let turn_id = id();
    assert!(matches!(
        store
            .begin_workflow_model(
                &project.id,
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
    let turn = store
        .finish_workflow_model(
            &project.id,
            &run.id,
            &lease,
            &claim,
            &turn_id,
            WorkflowModelResponse {
            received_cost_metadata: None,
                content: "Exact question".into(),
                tools: vec![ToolCall {
                    id: "q".into(),
                    name: tool.into(),
                    arguments,
                }],
                usage: Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(2),
                    ..Default::default()
                },
                rejection_code: None,
            },
        )
        .await
        .unwrap();
    let waiting = store
        .execute_workflow_artifact_tool(&project.id, &run.id, &lease, &turn.id, "q")
        .await
        .unwrap();
    (project.id, run.id, waiting)
}
async fn question(store: &Store) -> Question {
    let (project, run, waiting) = Box::pin(proposal(
        store,
        "request_input",
        json!({"question":"Keep exact Unicode and newlines?"}),
    ))
    .await;
    let WorkflowToolExecution::AwaitingInput(view) = waiting else {
        panic!("question")
    };
    Question { project, run, view }
}
async fn new_store() -> (Store, Arc<TestUtc>) {
    let mut store = Store::connect("sqlite::memory:").await.unwrap();
    let source = Arc::new(TestUtc::default());
    store.workflow_clock = WorkflowClock::for_test(source.clone());
    (store, source)
}
fn save_request(q: &Question, key: &str, base: i64) -> SaveWorkflowInputDraft {
    SaveWorkflowInputDraft {
        fingerprint: q.view.fingerprint.clone(),
        base_revision: base,
        idempotency_key: key.into(),
        answer: "  λ😀\r\nAntwort\n".into(),
    }
}
fn prepare_request(q: &Question, key: &str) -> PrepareWorkflowInputAnswer {
    PrepareWorkflowInputAnswer {
        fingerprint: q.view.fingerprint.clone(),
        base_draft_revision: 1,
        idempotency_key: key.into(),
    }
}
async fn snapshot(store: &Store, q: &Question) -> String {
    let mut values = Vec::new();
    for table in [
        "workflow_input_drafts",
        "workflow_input_draft_snapshots",
        "workflow_input_draft_saves",
        "workflow_input_answer_intents",
    ] {
        let rows: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT payload FROM {table} WHERE run_id=$1 ORDER BY payload"
        ))
        .bind(&q.run)
        .fetch_all(&store.pool)
        .await
        .unwrap();
        values.push(json!({"table":table,"rows":rows}));
    }
    let quota = sqlx::query("SELECT heads,snapshots,snapshot_bytes,intents FROM workflow_input_quotas WHERE project_id=$1")
        .bind(&q.project).fetch_optional(&store.pool).await.unwrap().map(|row|
            ["heads","snapshots","snapshot_bytes","intents"].map(|column| row.try_get::<i64,_>(column).unwrap()));
    let clock: Option<String> =
        sqlx::query_scalar("SELECT payload FROM workflow_execution_clocks WHERE run_id=$1")
            .bind(&q.run)
            .fetch_optional(&store.pool)
            .await
            .unwrap();
    canonical_hash(
        &json!({"input":values,"quota":quota,"clock":clock,"run":store.detail(&q.run).await.unwrap().run}),
    )
}
async fn highwater(store: &Store, q: &Question) -> i64 {
    sqlx::query_scalar("SELECT highwater_ms FROM workflow_execution_clocks WHERE run_id=$1")
        .bind(&q.run)
        .fetch_one(&store.pool)
        .await
        .unwrap()
}
async fn fixed_highwater(store: &Store, q: &Question) -> i64 {
    // Explicit stored test fixture, in the middle of a future second, so a
    // one-millisecond rollback cannot be mistaken for a seconds-only guard.
    let value = (highwater(store, q).await / 1000 + 2) * 1000 + 500;
    let payload: String =
        sqlx::query_scalar("SELECT payload FROM workflow_execution_clocks WHERE run_id=$1")
            .bind(&q.run)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    let mut clock: Value = serde_json::from_str(&payload).unwrap();
    clock["highwater_ms"] = json!(value);
    sqlx::query("UPDATE workflow_execution_clocks SET highwater_ms=$1,payload=$2 WHERE run_id=$3")
        .bind(value)
        .bind(serde_json::to_string(&clock).unwrap())
        .bind(&q.run)
        .execute(&store.pool)
        .await
        .unwrap();
    value
}
async fn save_confirmed(store: &Store, q: &Question) -> WorkflowInputDraftSave {
    store
        .save_workflow_input_draft(&q.run, &q.view.request_id, &save_request(q, "save", 0))
        .await
        .unwrap()
}
async fn prepare_confirmed(store: &Store, q: &Question) -> WorkflowInputAnswerIntent {
    store
        .prepare_workflow_input_answer(&q.run, &q.view.request_id, &prepare_request(q, "prepare"))
        .await
        .unwrap()
}
async fn delete_clock(store: &Store, q: &Question) {
    sqlx::query("DELETE FROM workflow_execution_clocks WHERE run_id=$1")
        .bind(&q.run)
        .execute(&store.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn missing_clock_blocks_new_input_writes_but_preserves_exact_receipt_replays() {
    let (store, _) = new_store().await;
    let q = Box::pin(question(&store)).await;
    let saved = save_confirmed(&store, &q).await;
    let prepared = prepare_confirmed(&store, &q).await;
    delete_clock(&store, &q).await;
    let before = snapshot(&store, &q).await;
    assert!(matches!(
        store
            .save_workflow_input_draft(&q.run, &q.view.request_id, &save_request(&q, "new", 1))
            .await,
        Err(Error::WorkflowClockUnverified)
    ));
    assert!(matches!(
        store
            .prepare_workflow_input_answer(&q.run, &q.view.request_id, &prepare_request(&q, "new"))
            .await,
        Err(Error::WorkflowClockUnverified)
    ));
    assert_eq!(
        canonical_hash(&saved),
        canonical_hash(
            &store
                .save_workflow_input_draft(&q.run, &q.view.request_id, &save_request(&q, "save", 0))
                .await
                .unwrap()
        )
    );
    assert_eq!(
        canonical_hash(&prepared),
        canonical_hash(
            &store
                .prepare_workflow_input_answer(
                    &q.run,
                    &q.view.request_id,
                    &prepare_request(&q, "prepare")
                )
                .await
                .unwrap()
        )
    );
    assert_eq!(before, snapshot(&store, &q).await);
    let view = store.workflow_run_view(&q.project, &q.run).await.unwrap();
    assert!(!view.inputs[0].actionable);
    assert_eq!(
        view.inputs[0].closed_reason.as_deref(),
        Some("workflow_clock_unverified")
    );
    assert!(!view.completion_resume_available);
    assert_eq!(
        store
            .workflow_input_draft(&q.run, &q.view.request_id)
            .await
            .unwrap()
            .draft
            .answer,
        save_request(&q, "", 0).answer
    );
    assert_eq!(
        before,
        snapshot(&store, &q).await,
        "read cannot seed an old clock"
    );
}
#[tokio::test]
async fn millisecond_rollback_blocks_new_writes_and_input_actionability_without_advancing_clock() {
    let (store, clock) = new_store().await;
    let q = Box::pin(question(&store)).await;
    let saved = save_confirmed(&store, &q).await;
    let prepared = prepare_confirmed(&store, &q).await;
    let watermark = fixed_highwater(&store, &q).await;
    assert_eq!(watermark / 1000, (watermark - 1) / 1000);
    let before = snapshot(&store, &q).await;
    clock.samples([watermark - 1]);
    assert!(matches!(
        store
            .save_workflow_input_draft(&q.run, &q.view.request_id, &save_request(&q, "new", 1))
            .await,
        Err(Error::WorkflowClockRollback)
    ));
    clock.samples([watermark - 1]);
    assert!(matches!(
        store
            .prepare_workflow_input_answer(&q.run, &q.view.request_id, &prepare_request(&q, "new"))
            .await,
        Err(Error::WorkflowClockRollback)
    ));
    assert_eq!(
        canonical_hash(&saved),
        canonical_hash(
            &store
                .save_workflow_input_draft(&q.run, &q.view.request_id, &save_request(&q, "save", 0))
                .await
                .unwrap()
        )
    );
    assert_eq!(
        canonical_hash(&prepared),
        canonical_hash(
            &store
                .prepare_workflow_input_answer(
                    &q.run,
                    &q.view.request_id,
                    &prepare_request(&q, "prepare")
                )
                .await
                .unwrap()
        )
    );
    clock.samples([watermark - 1]);
    let view = store.workflow_run_view(&q.project, &q.run).await.unwrap();
    assert!(!view.inputs[0].actionable);
    assert_eq!(
        view.inputs[0].closed_reason.as_deref(),
        Some("workflow_clock_rollback")
    );
    assert_eq!(before, snapshot(&store, &q).await);
}
#[tokio::test]
async fn final_save_sample_rolls_back_every_sql_effect_and_quota() {
    let (store, clock) = new_store().await;
    let q = Box::pin(question(&store)).await;
    let before = snapshot(&store, &q).await;
    let watermark = highwater(&store, &q).await;
    clock.samples([watermark, watermark - 1]);
    assert!(matches!(
        store
            .save_workflow_input_draft(
                &q.run,
                &q.view.request_id,
                &save_request(&q, "final-save", 0)
            )
            .await,
        Err(Error::WorkflowClockRollback)
    ));
    assert_eq!(before, snapshot(&store, &q).await);
    assert_eq!(
        store
            .workflow_input_draft(&q.run, &q.view.request_id)
            .await
            .unwrap()
            .draft
            .revision,
        0
    );
}
#[tokio::test]
async fn final_prepare_sample_rolls_back_intent_and_quota() {
    let (store, clock) = new_store().await;
    let q = Box::pin(question(&store)).await;
    save_confirmed(&store, &q).await;
    let before = snapshot(&store, &q).await;
    let watermark = highwater(&store, &q).await;
    clock.samples([watermark, watermark - 1]);
    assert!(matches!(
        store
            .prepare_workflow_input_answer(
                &q.run,
                &q.view.request_id,
                &prepare_request(&q, "final-prepare")
            )
            .await,
        Err(Error::WorkflowClockRollback)
    ));
    assert_eq!(before, snapshot(&store, &q).await);
    assert!(
        store
            .workflow_input_draft(&q.run, &q.view.request_id)
            .await
            .unwrap()
            .intents
            .is_empty()
    );
}
#[tokio::test]
async fn final_execution_sample_enforces_original_question_deadline() {
    let (store, clock) = new_store().await;
    let q = Box::pin(question(&store)).await;
    let before = snapshot(&store, &q).await;
    clock.samples([highwater(&store, &q).await, q.view.expires_at]);
    assert!(matches!(
        store
            .save_workflow_input_draft(
                &q.run,
                &q.view.request_id,
                &save_request(&q, "expired-save", 0)
            )
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(before, snapshot(&store, &q).await);
    save_confirmed(&store, &q).await;
    let before = snapshot(&store, &q).await;
    clock.samples([highwater(&store, &q).await, q.view.expires_at]);
    assert!(matches!(
        store
            .prepare_workflow_input_answer(
                &q.run,
                &q.view.request_id,
                &prepare_request(&q, "expired-prepare")
            )
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(before, snapshot(&store, &q).await);
}
#[tokio::test]
async fn corrupted_projection_is_forbidden_and_accepted_receipt_remains_read_only() {
    let (store, _) = new_store().await;
    let q = Box::pin(question(&store)).await;
    let saved = save_confirmed(&store, &q).await;
    let prepared = prepare_confirmed(&store, &q).await;
    sqlx::query("UPDATE workflow_execution_clocks SET payload='{}' WHERE run_id=$1")
        .bind(&q.run)
        .execute(&store.pool)
        .await
        .unwrap();
    let before = snapshot(&store, &q).await;
    assert!(matches!(
        store
            .save_workflow_input_draft(&q.run, &q.view.request_id, &save_request(&q, "new", 1))
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .prepare_workflow_input_answer(&q.run, &q.view.request_id, &prepare_request(&q, "new"))
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store.workflow_run_view(&q.project, &q.run).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        canonical_hash(&saved),
        canonical_hash(
            &store
                .save_workflow_input_draft(&q.run, &q.view.request_id, &save_request(&q, "save", 0))
                .await
                .unwrap()
        )
    );
    assert_eq!(
        canonical_hash(&prepared),
        canonical_hash(
            &store
                .prepare_workflow_input_answer(
                    &q.run,
                    &q.view.request_id,
                    &prepare_request(&q, "prepare")
                )
                .await
                .unwrap()
        )
    );
    assert_eq!(before, snapshot(&store, &q).await);
    let (store, _) = new_store().await;
    let q = Box::pin(question(&store)).await;
    save_confirmed(&store, &q).await;
    let prepared = prepare_confirmed(&store, &q).await;
    let accepted = store
        .commit_workflow_input_answer_intent(&q.run, &prepared.id)
        .await
        .unwrap();
    assert_eq!(accepted.state, WorkflowInputAnswerIntentState::Accepted);
    delete_clock(&store, &q).await;
    let before = snapshot(&store, &q).await;
    assert_eq!(
        canonical_hash(&accepted),
        canonical_hash(
            &store
                .commit_workflow_input_answer_intent(&q.run, &prepared.id)
                .await
                .unwrap()
        )
    );
    let view = store.workflow_run_view(&q.project, &q.run).await.unwrap();
    assert_eq!(view.inputs[0].state, WorkflowInputState::Answered);
    assert_eq!(
        canonical_hash(&view.inputs[0].receipt),
        canonical_hash(&accepted.receipt)
    );
    assert_eq!(before, snapshot(&store, &q).await);
}

#[tokio::test]
#[ignore = "Requires a dedicated COWORK_TEST_POSTGRES database"]
async fn postgres_input_clock_fences_after_actual_run_and_quota_lock_waits() {
    let url = std::env::var("COWORK_TEST_POSTGRES").unwrap();
    let mut store = Store::connect(&url).await.unwrap();
    let clock = Arc::new(TestUtc::default());
    store.workflow_clock = WorkflowClock::for_test(clock.clone());
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    for (prepare, quota_lock) in [(false, false), (false, true), (true, false), (true, true)] {
        clock.samples([]);
        let q = Box::pin(question(&store)).await;
        save_confirmed(&store, &q).await;
        let before = snapshot(&store, &q).await;
        let watermark = highwater(&store, &q).await;
        let mut lock = pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        if quota_lock {
            sqlx::query("UPDATE workflow_input_quotas SET heads=heads WHERE project_id=$1")
                .bind(&q.project)
                .execute(&mut *lock)
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE runs SET revision=revision WHERE id=$1")
                .bind(&q.run)
                .execute(&mut *lock)
                .await
                .unwrap();
        }
        let copy = store.clone();
        let run = q.run.clone();
        let request = q.view.request_id.clone();
        let save = save_request(&q, "blocked-save", 1);
        let intent = prepare_request(&q, "blocked-prepare");
        let task = tokio::spawn(async move {
            if prepare {
                copy.prepare_workflow_input_answer(&run, &request, &intent)
                    .await
                    .map(|_| ())
            } else {
                copy.save_workflow_input_draft(&run, &request, &save)
                    .await
                    .map(|_| ())
            }
        });
        tokio::time::timeout(Duration::from_secs(4),async {
            loop {
                let blocked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid))")
                    .bind(blocker).fetch_one(&pool).await.unwrap();
                if blocked > 0 { break; }
                assert!(!task.is_finished(),"the actual draft/prepare transaction must reach its owned lock wait");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        clock.samples([watermark - 1; 4]);
        lock.commit().await.unwrap();
        assert!(matches!(
            task.await.unwrap(),
            Err(Error::WorkflowClockRollback)
        ));
        clock.samples([]);
        assert_eq!(
            before,
            snapshot(&store, &q).await,
            "all transaction writes and quota must roll back after the observed lock wait"
        );
    }
    pool.close().await;
    store.close().await;
}

#[tokio::test]
async fn validated_rejection_uses_monotone_evidence_under_earlier_utc() {
    let (store, clock) = new_store().await;
    let q = Box::pin(question(&store)).await;
    save_confirmed(&store, &q).await;
    // A valid, future test sample proves that the prior confirmed intent time
    // is ahead of the current system sample; no OS/global clock is changed.
    let accepted_time = now() + 5_000;
    assert!(accepted_time < q.view.expires_at);
    clock.samples([accepted_time, accepted_time]);
    let pending = prepare_confirmed(&store, &q).await;
    assert_eq!(pending.created_at, accepted_time);
    let frozen = canonical_hash(
        &json!({"submission":pending.submission,"snapshot":pending.draft_snapshot,"request_sha256":pending.request_sha256}),
    );
    let watermark = highwater(&store, &q).await;
    clock.samples([watermark - 1; 32]);
    store.cancel(&q.run).await.unwrap();
    assert!(
        now() < pending.created_at,
        "the previous now()-only closure would produce an unreadable receipt"
    );
    let rejected = store
        .commit_workflow_input_answer_intent(&q.run, &pending.id)
        .await
        .unwrap();
    assert_eq!(rejected.state, WorkflowInputAnswerIntentState::Rejected);
    assert_eq!(rejected.rejection.as_ref().unwrap().code, "input_closed");
    assert!(rejected.updated_at >= pending.created_at);
    assert_eq!(
        frozen,
        canonical_hash(
            &json!({"submission":rejected.submission,"snapshot":rejected.draft_snapshot,"request_sha256":rejected.request_sha256})
        )
    );
    let reread = store
        .workflow_input_answer_intent(&q.run, &pending.id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&reread), canonical_hash(&rejected));
    assert_eq!(
        canonical_hash(
            &store
                .commit_workflow_input_answer_intent(&q.run, &pending.id)
                .await
                .unwrap()
        ),
        canonical_hash(&rejected)
    );
}

#[tokio::test]
async fn input_write_fences_reject_rollback_above_the_durable_run_highwater() {
    let (store, clock) = new_store().await;
    let q = Box::pin(question(&store)).await;
    let watermark = highwater(&store, &q).await;
    let before = snapshot(&store, &q).await;
    clock.samples([watermark + 2, watermark + 1]);
    assert!(matches!(
        store
            .save_workflow_input_draft(
                &q.run,
                &q.view.request_id,
                &save_request(&q, "intratransaction-save", 0)
            )
            .await,
        Err(Error::WorkflowClockRollback)
    ));
    assert_eq!(before, snapshot(&store, &q).await);
    save_confirmed(&store, &q).await;
    let before = snapshot(&store, &q).await;
    clock.samples([watermark + 2, watermark + 1]);
    assert!(matches!(
        store
            .prepare_workflow_input_answer(
                &q.run,
                &q.view.request_id,
                &prepare_request(&q, "intratransaction-prepare")
            )
            .await,
        Err(Error::WorkflowClockRollback)
    ));
    assert_eq!(before, snapshot(&store, &q).await);
}

#[tokio::test]
async fn clock_denial_hides_actionable_tool_approval_but_preserves_its_exact_record() {
    for missing in [true, false] {
        let (store, clock) = new_store().await;
        let (project, run, waiting) = Box::pin(proposal(
            &store,
            "create_artifact",
            json!({
            "name":"must-not-execute.txt","content":"preserve exact proposal"}),
        ))
        .await;
        let WorkflowToolExecution::AwaitingApproval(approval) = waiting else {
            panic!("exact artifact approval required")
        };
        let live = store.workflow_run_view(&project, &run).await.unwrap();
        assert_eq!(live.approvals.len(), 1);
        assert_eq!(
            canonical_hash(&live.approvals[0].approval),
            canonical_hash(&approval)
        );
        let before = canonical_hash(&store.stored_run(&run).await.unwrap());
        let watermark: i64 = sqlx::query_scalar(
            "SELECT highwater_ms FROM workflow_execution_clocks WHERE run_id=$1",
        )
        .bind(&run)
        .fetch_one(&store.pool)
        .await
        .unwrap();
        if missing {
            sqlx::query("DELETE FROM workflow_execution_clocks WHERE run_id=$1")
                .bind(&run)
                .execute(&store.pool)
                .await
                .unwrap();
        } else {
            clock.samples([watermark - 1]);
        }
        let view = store.workflow_run_view(&project, &run).await.unwrap();
        assert!(view.approvals.is_empty());
        assert_eq!(
            before,
            canonical_hash(&store.stored_run(&run).await.unwrap())
        );
        clock.samples([watermark - 1]);
        let refused = store
            .resolve_approval(
                &run,
                &approval.id,
                ResolveApproval {
                    fingerprint: approval.fingerprint.clone(),
                    allow: true,
                },
            )
            .await;
        assert!(matches!(
            (&refused, missing),
            (Err(Error::WorkflowClockUnverified), true)
                | (Err(Error::WorkflowClockRollback), false)
        ));
        assert_eq!(
            before,
            canonical_hash(&store.stored_run(&run).await.unwrap())
        );
        let detail = store.detail(&run).await.unwrap();
        assert_eq!(detail.approvals.len(), 1);
        assert_eq!(
            canonical_hash(&detail.approvals[0]),
            canonical_hash(&approval)
        );
        assert!(detail.artifacts.is_empty());
    }
}
