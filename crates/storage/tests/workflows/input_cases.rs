use super::*;
use cowork_domain::canonical_hash;
use cowork_storage::{
    WorkflowModelAdmission, WorkflowModelResponse, WorkflowModelTurn, WorkflowToolExecution,
};
use cowork_workflow_contracts::*;
use serde_json::Value;
use std::time::Duration;

pub(super) async fn input_fixture(
    store: &Store,
    access: AccessMode,
) -> (Fixture, RunRecord, String) {
    let mut f = tool_cases::tool_fixture(store).await;
    f.definition.rights.tools.insert("request_input".into());
    for role in &mut f.definition.roles {
        role.rights.tools.insert("request_input".into());
    }
    for step in &mut f.definition.steps {
        step.rights.tools.insert("request_input".into());
    }
    if !f
        .spec
        .allowed_tools
        .iter()
        .any(|tool| tool == "request_input")
    {
        f.spec.allowed_tools.push("request_input".into());
    }
    f.spec.access_mode = access;
    let version = store
        .publish_workflow(
            &f.spec.project_id,
            &f.request.workflow_id,
            2,
            "input-fixture",
            &f.definition,
        )
        .await
        .unwrap();
    f.request.version_id = version.version_id.unwrap();
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    (f, run, parent.lease.unwrap().token)
}
async fn input_catalog(store: &Store, f: &Fixture, run: &RunRecord, node: &str) -> Value {
    let spec = store
        .workflow_node_spec(&f.spec.project_id, &run.id, node)
        .await
        .unwrap();
    cowork_domain::model_tool_catalog(&spec)
}
pub(super) async fn propose(
    store: &Store,
    f: &Fixture,
    run: &RunRecord,
    lease: &str,
    node: &str,
    arguments: Value,
) -> (WorkflowClaim, WorkflowModelTurn) {
    let claim = claim_node(store, f, run, lease, node).await;
    let (revision, context) = store
        .workflow_model_context(
            &f.spec.project_id,
            &run.id,
            &claim,
            input_catalog(store, f, run, node).await,
        )
        .await
        .unwrap();
    let turn_id = id();
    assert!(matches!(
        store
            .begin_workflow_model(
                &f.spec.project_id,
                &run.id,
                lease,
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
            &f.spec.project_id,
            &run.id,
            lease,
            &claim,
            &turn_id,
            WorkflowModelResponse {
                provider_continuation: None,
                received_cost_metadata: None,
                content: "Confirmed input proposal".into(),
                tools: vec![ToolCall {
                    id: "same-provider-call".into(),
                    name: "request_input".into(),
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
    (claim, turn)
}
pub(super) async fn opened(
    store: &Store,
    f: &Fixture,
    run: &RunRecord,
    lease: &str,
    turn: &WorkflowModelTurn,
) -> WorkflowInputView {
    let WorkflowToolExecution::AwaitingInput(q) = store
        .execute_workflow_artifact_tool(
            &f.spec.project_id,
            &run.id,
            lease,
            &turn.id,
            "same-provider-call",
        )
        .await
        .unwrap()
    else {
        panic!("input must wait");
    };
    *q
}
async fn intent(
    store: &Store,
    run: &RunRecord,
    q: &WorkflowInputView,
    text: &str,
) -> WorkflowInputAnswerIntent {
    let lookup = store
        .workflow_input_draft(&run.id, &q.request_id)
        .await
        .unwrap();
    let saved = store
        .save_workflow_input_draft(
            &run.id,
            &q.request_id,
            &SaveWorkflowInputDraft {
                fingerprint: q.fingerprint.clone(),
                base_revision: lookup.draft.revision,
                idempotency_key: id(),
                answer: text.into(),
            },
        )
        .await
        .unwrap();
    let WorkflowInputDraftSave::Saved { draft, .. } = saved else {
        panic!("fresh draft must save")
    };
    let prepared = store
        .prepare_workflow_input_answer(
            &run.id,
            &q.request_id,
            &PrepareWorkflowInputAnswer {
                fingerprint: q.fingerprint.clone(),
                base_draft_revision: draft.revision,
                idempotency_key: id(),
            },
        )
        .await
        .unwrap();
    assert_eq!(prepared.submission.input.answer, text);
    prepared
}
async fn contract(store: &Store) {
    let (f, run, lease) = input_fixture(store, AccessMode::ConfirmMutations).await;
    let (a, turn_a) = propose(
        store,
        &f,
        &run,
        &lease,
        "a",
        json!({"question":"Which source for A?"}),
    )
    .await;
    let (_, turn_b) = propose(
        store,
        &f,
        &run,
        &lease,
        "b",
        json!({"question":"Which source for B?"}),
    )
    .await;
    let parent_seed = store.stored_run(&run.id).await.unwrap().checkpoint.messages;
    let qa = opened(store, &f, &run, &lease, &turn_a).await;
    let qb = opened(store, &f, &run, &lease, &turn_b).await;
    assert_ne!(qa.request_id, qb.request_id);
    assert_ne!(qa.fingerprint, qb.fingerprint);
    let before = store.stored_run(&run.id).await.unwrap();
    assert_eq!(before.record.state, RunState::Running);
    assert_eq!(before.lease.as_ref().unwrap().token, lease);
    assert_eq!(before.record.steps, 4);
    let repeat = opened(store, &f, &run, &lease, &turn_a).await;
    assert_eq!(repeat.request_id, qa.request_id);
    assert_eq!(
        store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .last_sequence,
        before.record.last_sequence
    );
    let view = store
        .workflow_run_view(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert_eq!(view.inputs.iter().filter(|q| q.actionable).count(), 2);
    assert!(!view.input_resume_available);
    assert!(
        view.nodes
            .iter()
            .filter(|n| n.id == "a" || n.id == "b")
            .all(|n| n.state == WorkflowNodeStatus::WaitingInput)
    );
    assert!(matches!(
        store
            .submit_input(
                &run.id,
                SubmitInput {
                    request_id: qa.request_id.clone(),
                    answer: "No legacy workflow write".into()
                }
            )
            .await,
        Err(Error::Forbidden)
    ));
    let graph = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let mut corrupt = graph.clone();
    corrupt.inputs.get_mut(&qa.request_id).unwrap().expires_at += 1;
    assert!(matches!(
        corrupt.validate(&before.record),
        Err(Error::Forbidden)
    ));
    let exact = "  unver\u{e4}ndert \"quoted\" \\\r\nUnicode: \u{f6}\u{df}\u{1f600}\n  ";
    let ia = intent(store, &run, &qa, exact).await;
    let ib = intent(store, &run, &qb, "Independent B").await;
    let (ra, rb) = tokio::join!(
        store.commit_workflow_input_answer_intent(&run.id, &ia.id),
        store.commit_workflow_input_answer_intent(&run.id, &ib.id),
    );
    let ra = ra.unwrap();
    let rb = rb.unwrap();
    assert_eq!(ra.state, WorkflowInputAnswerIntentState::Accepted);
    assert_eq!(rb.state, WorkflowInputAnswerIntentState::Accepted);
    let after = store.stored_run(&run.id).await.unwrap();
    assert_eq!(after.checkpoint.messages, parent_seed);
    assert_eq!(after.record.state, RunState::Running);
    assert_eq!(after.lease.unwrap().token, lease);
    assert_eq!(after.record.steps, 4);
    assert_eq!(after.record.last_sequence, before.record.last_sequence + 2);
    let original = ra.receipt.unwrap();
    let repeated = store
        .commit_workflow_input_answer_intent(&run.id, &ia.id)
        .await
        .unwrap();
    assert_eq!(repeated.receipt.as_ref(), Some(&original));
    assert_eq!(
        store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .last_sequence,
        after.record.last_sequence
    );
    let current = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let tool = current.agents["a"].turns[0].tool_results[0].clone();
    assert_eq!(tool.result["answer"], exact);
    assert_eq!(
        current.agents["b"].turns[0].tool_results[0].result["answer"],
        "Independent B"
    );
    let (_, context) = store
        .workflow_model_context(
            &f.spec.project_id,
            &run.id,
            &a,
            input_catalog(store, &f, &run, &a.node_id).await,
        )
        .await
        .unwrap();
    assert_eq!(
        context
            .messages
            .iter()
            .find(|m| m["role"] == "tool")
            .unwrap()["content"],
        exact
    );
    assert!(
        !context
            .messages
            .iter()
            .any(|m| m["content"] == "Independent B")
    );
    let events = store.events(&run.id, 0).await.unwrap();
    let answered: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "workflow_input_answered")
        .collect();
    assert_eq!(answered.len(), 2);
    assert!(
        answered
            .iter()
            .all(|e| e.payload.get("answer").is_none() && e.payload.get("receipt").is_none())
    );
    assert!(
        answered
            .iter()
            .any(|e| e.sequence == original.confirmed_sequence
                && e.payload["answer_sha256"] == original.answer_sha256)
    );
    let mut bad = current.clone();
    bad.inputs
        .get_mut(&qa.request_id)
        .unwrap()
        .receipt
        .as_mut()
        .unwrap()
        .confirmed_sequence = after.record.last_sequence + 1;
    assert!(matches!(bad.validate(&after.record), Err(Error::Forbidden)));
    let export = store.export_order(&run.spec.order_id).await.unwrap();
    assert_eq!(export.format_version, 13);
    // Real durable answer-intent reads must retain the original text and receipt.
    assert_eq!(
        store
            .workflow_input_answer_intent(&run.id, &ia.id)
            .await
            .unwrap()
            .submission
            .input
            .answer,
        exact
    );
    store.cancel(&run.id).await.unwrap();
}
async fn confirm_all_and_revocation(store: &Store) {
    let (f, run, lease) = input_fixture(store, AccessMode::ConfirmAll).await;
    let (_, turn) = propose(
        store,
        &f,
        &run,
        &lease,
        "a",
        json!({"question":"Approval first"}),
    )
    .await;
    let WorkflowToolExecution::AwaitingApproval(a) = store
        .execute_workflow_artifact_tool(
            &f.spec.project_id,
            &run.id,
            &lease,
            &turn.id,
            "same-provider-call",
        )
        .await
        .unwrap()
    else {
        panic!("confirm_all")
    };
    assert!(
        store
            .stored_workflow(&f.spec.project_id, &run.id)
            .await
            .unwrap()
            .inputs
            .is_empty()
    );
    store
        .resolve_approval(
            &run.id,
            &a.id,
            ResolveApproval {
                fingerprint: a.fingerprint,
                allow: true,
            },
        )
        .await
        .unwrap();
    let q = opened(store, &f, &run, &lease, &turn).await;
    let pending = intent(store, &run, &q, "Retained after revocation").await;
    store
        .revoke_workflow(
            &f.spec.project_id,
            &f.request.workflow_id,
            3,
            "revoke-input",
        )
        .await
        .unwrap();
    let committed = store
        .commit_workflow_input_answer_intent(&run.id, &pending.id)
        .await;
    match committed {
        Ok(i) => assert_ne!(i.state, WorkflowInputAnswerIntentState::Accepted),
        Err(e) => assert!(matches!(
            e,
            Error::Forbidden | Error::WorkflowRevoked | Error::Conflict
        )),
    }
    let after = store
        .workflow_run_view(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert_eq!(after.inputs[0].state, WorkflowInputState::Closed);
    assert!(!after.inputs[0].actionable);
    assert!(
        store
            .stored_workflow(&f.spec.project_id, &run.id)
            .await
            .unwrap()
            .inputs[&q.request_id]
            .receipt
            .is_none()
    );
    store.cancel(&run.id).await.unwrap();
}
async fn malformed_proposal(store: &Store) {
    let (f, run, lease) = input_fixture(store, AccessMode::ReadOnly).await;
    let (_, turn) = propose(
        store,
        &f,
        &run,
        &lease,
        "a",
        json!({"question":"Do not change my scope","grant":"autonomous"}),
    )
    .await;
    let before = store.stored_run(&run.id).await.unwrap();
    assert!(matches!(
        store
            .execute_workflow_artifact_tool(
                &f.spec.project_id,
                &run.id,
                &lease,
                &turn.id,
                "same-provider-call"
            )
            .await,
        Err(Error::Validation(_))
    ));
    let after = store.stored_run(&run.id).await.unwrap();
    assert_eq!(after.record.last_sequence, before.record.last_sequence);
    assert_eq!(after.record.steps, before.record.steps);
    assert!(
        store
            .stored_workflow(&f.spec.project_id, &run.id)
            .await
            .unwrap()
            .inputs
            .is_empty()
    );
    store.cancel(&run.id).await.unwrap();
}
#[tokio::test]
async fn sqlite_workflow_input_contract() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    contract(&store).await;
    confirm_all_and_revocation(&store).await;
    malformed_proposal(&store).await;
    store.close().await;
}
#[tokio::test]
#[ignore = "Requires dedicated COWORK_TEST_POSTGRES database"]
async fn postgres_workflow_input_contract() {
    let url = std::env::var("COWORK_TEST_POSTGRES").unwrap();
    let store = Store::connect(&url).await.unwrap();
    contract(&store).await;
    confirm_all_and_revocation(&store).await;
    malformed_proposal(&store).await;
    postgres_input_commit_rechecks_lease(&store, &url).await;
    store.close().await;
}
async fn postgres_input_commit_rechecks_lease(store: &Store, url: &str) {
    let (f, run, lease) = input_fixture(store, AccessMode::Autonomous).await;
    let (_, turn) = propose(
        store,
        &f,
        &run,
        &lease,
        "a",
        json!({"question":"Lease fence"}),
    )
    .await;
    let q = opened(store, &f, &run, &lease, &turn).await;
    let frozen = intent(store, &run, &q, "Must not commit after lock expiry").await;
    let before = store.stored_run(&run.id).await.unwrap();
    let graph = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let pool = sqlx::PgPool::connect(url).await.unwrap();
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
    let run_id = run.id.clone();
    let intent_id = frozen.id.clone();
    let task = tokio::spawn(async move {
        owned
            .commit_workflow_input_answer_intent(&run_id, &intent_id)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5),async {
        loop {
            let blocked:i64=sqlx::query_scalar("SELECT COUNT(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'UPDATE workflows%' AND $1=ANY(pg_blocking_pids(pid))").bind(blocker).fetch_one(&pool).await.unwrap();
            if blocked>0 {break;}
            assert!(!task.is_finished(),"input must reach its own blocked registry gate");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(15_200)).await;
    assert!(!task.is_finished());
    lock.commit().await.unwrap();
    match task.await.unwrap() {
        Ok(i) => assert_ne!(i.state, WorkflowInputAnswerIntentState::Accepted),
        Err(e) => assert!(matches!(e, Error::StaleLease)),
    }
    let after = store.stored_run(&run.id).await.unwrap();
    assert_eq!(after.record.last_sequence, before.record.last_sequence);
    assert_eq!(after.record.steps, before.record.steps);
    assert_eq!(
        canonical_hash(
            &store
                .stored_workflow(&f.spec.project_id, &run.id)
                .await
                .unwrap()
        ),
        canonical_hash(&graph)
    );
    assert!(store.recover_expired_leases().await.unwrap() > 0);
    let view = store
        .workflow_run_view(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert!(view.input_resume_available);
    assert!(view.inputs.iter().all(|q| !q.actionable));
    assert!(store.resume(&run.id).await.is_err());
    store.cancel(&run.id).await.unwrap();
    pool.close().await;
}
