use super::*;
use cowork_domain::workflows::*;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicI64, Ordering};

struct ManualUtc(AtomicI64);
impl ManualUtc {
    fn new() -> Arc<Self> {
        Arc::new(Self(AtomicI64::new(now())))
    }
    fn set(&self, value: i64) {
        self.0.store(value, Ordering::SeqCst);
    }
}
impl UtcSource for ManualUtc {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct Fixture {
    request: CreateWorkflowRun,
    spec: RunSpec,
    run: RunRecord,
    provider: ProviderProfile,
}
fn install(store: &mut Store) -> Arc<ManualUtc> {
    let source = ManualUtc::new();
    store.workflow_clock = WorkflowClock::for_test(source.clone());
    source
}
async fn fixture(store: &Store) -> Fixture {
    let project = store
        .create_project(CreateProject {
            name: "Private clock fixture".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Clock fixture".into(),
        })
        .await
        .unwrap();
    let provider = ProviderProfile {
        generation: Default::default(),
        id: id(),
        name: "Deterministic test profile".into(),
        kind: ProviderKind::Test,
        endpoint: String::new(),
        model: "clock-fixture".into(),
        revision: 1,
        context_tokens: 2048,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    store.insert_provider(&provider).await.unwrap();
    let budgets = Budgets {
        max_steps: 8,
        max_seconds: 60,
        max_output_tokens: 256,
        max_total_tokens: 4096,
    };
    let rights = WorkflowRights {
        current_workspace: false,
        tools: BTreeSet::from(["request_input".into()]),
        folders: BTreeSet::new(),
    };
    let definition = WorkflowDefinition {
        documents: Default::default(),
        iterations: Default::default(),
        subflows: Default::default(),
        controls: Default::default(),
        gates: Default::default(),
        dataflow: None,
        format_version: 1,
        name: "Clock contract".into(),
        mode: WorkflowMode::Sequential,
        failure: WorkflowFailure::ContinueIndependent,
        rights: rights.clone(),
        budgets: budgets.clone(),
        roles: vec![WorkflowRole {
            agent_config: None,
            model_binding: None,
            output: None,
            id: "author".into(),
            instructions: "Produce a synthetic answer".into(),
            provider_id: provider.id.clone(),
            provider_revision: 1,
            rights: rights.clone(),
            budgets: budgets.clone(),
            delegate_to: BTreeSet::new(),
        }],
        steps: vec![WorkflowStep {
            execution: None,
            zero_context: false,
            id: "a".into(),
            role_id: "author".into(),
            goal: "Synthetic result".into(),
            dependencies: vec![],
            rights,
            budgets: budgets.clone(),
        }],
    };
    let library = store
        .create_workflow(&project.id, "create", &definition)
        .await
        .unwrap();
    let request = CreateWorkflowRun {
        order_id: order.id.clone(),
        base_revision: order.revision,
        idempotency_key: "start".into(),
        workflow_id: library.id,
        version_id: library.versions[0].id.clone(),
        max_parallel: 1,
    };
    let spec = RunSpec {
        input_workspace: None,
        agent_config: None,
        model_binding: None,
        output: None,
        workflow: None,
        mcp_tools: vec![],
        environment: None,
        folders: vec![],
        order_id: order.id,
        project_id: project.id,
        prompt: "Clock contract synthetic run".into(),
        provider: None,
        file_action: None,
        access_mode: AccessMode::ConfirmMutations,
        executor: "local-v2".into(),
        policy_revision: 1,
        allowed_tools: vec!["request_input".into()],
        budgets,
    };
    let run = store
        .create_workflow_run(&request, spec.clone())
        .await
        .unwrap();
    Fixture {
        request,
        spec,
        run,
        provider,
    }
}
async fn clock_record(store: &Store, run: &RunRecord) -> WorkflowClockRecord {
    let mut tx = store.pool.begin().await.unwrap();
    let value = store.workflow_clock.read(&mut tx, run).await.unwrap();
    tx.rollback().await.unwrap();
    match value {
        WorkflowClockRead::Verified(record) => *record,
        _ => panic!("Clock must be verified"),
    }
}
async fn execution(store: &Store, run: &RunRecord) -> Result<i64> {
    let mut tx = store.pool.begin().await.map_err(storage_error)?;
    store.workflow_clock.execution_now(&mut tx, run).await
}
async fn no_clock(store: &Store, run: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_execution_clocks WHERE run_id=$1")
        .bind(run)
        .fetch_one(&store.pool)
        .await
        .unwrap()
        == 0
}
async fn started(store: &Store, f: &Fixture) -> StoredRun {
    store
        .claim_workflow_run(&f.spec.project_id, &f.run.id)
        .await
        .unwrap()
}
async fn seed_replay_and_single_agent(store: &Store, source: &ManualUtc) {
    let f = fixture(store).await;
    let original = clock_record(store, &f.run).await;
    source.set(original.highwater_ms - 1);
    let replay = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    assert_eq!(canonical_hash(&f.run), canonical_hash(&replay));
    assert_eq!(
        canonical_hash(&original),
        canonical_hash(&clock_record(store, &replay).await)
    );
    assert!(matches!(
        execution(store, &replay).await,
        Err(Error::WorkflowClockRollback)
    ));
    source.set(original.highwater_ms);
    let order = store
        .create_order(CreateOrder {
            project_id: f.spec.project_id.clone(),
            title: "Single agent control".into(),
        })
        .await
        .unwrap();
    let mut spec = f.spec.clone();
    spec.order_id = order.id.clone();
    spec.provider = Some(f.provider);
    let input = CreateRun {
        agent_config: None,
        model_binding: None,
        output: None,
        native_tools: vec![],
        environment: None,
        order_id: order.id,
        base_revision: order.revision,
        idempotency_key: "single".into(),
        prompt: spec.prompt.clone(),
        provider_id: spec.provider.as_ref().unwrap().id.clone(),
        provider_revision: 1,
        access_mode: spec.access_mode.clone(),
        budgets: spec.budgets.clone(),
        folders: vec![],
        mcp_tools: vec![],
    };
    let single = store.create_run(&input, spec).await.unwrap();
    assert!(no_clock(store, &single.id).await);
    store.cancel(&single.id).await.unwrap();
    store.cancel(&f.run.id).await.unwrap();
}
async fn rollback_and_concurrent_mutations(store: &Store, source: &ManualUtc) {
    let f = fixture(store).await;
    let parent = started(store, &f).await;
    let lease = parent.lease.as_ref().unwrap().token.clone();
    let original = clock_record(store, &parent.record).await;
    source.set(original.highwater_ms + 2);
    let (a, b) = tokio::join!(
        store.heartbeat(&f.run.id, &lease),
        store.mutate(&f.run.id, Some(&lease), |_| Ok(Change::event(
            "clock_fixture_metadata",
            json!({"synthetic":true})
        )))
    );
    a.unwrap();
    b.unwrap();
    let before = store.stored_run(&f.run.id).await.unwrap();
    let clock = clock_record(store, &before.record).await;
    assert_eq!(clock.last_run_revision, before.record.revision);
    assert!(clock.highwater_ms >= original.highwater_ms);
    source.set(clock.highwater_ms - 1);
    assert!(matches!(
        store.heartbeat(&f.run.id, &lease).await,
        Err(Error::WorkflowClockRollback)
    ));
    assert!(matches!(
        store
            .mutate(&f.run.id, Some(&lease), |_| {
                Ok(Change {
                    artifacts: vec![Artifact {
                        id: id(),
                        run_id: f.run.id.clone(),
                        order_id: f.spec.order_id.clone(),
                        name: "must-not-commit".into(),
                        media_type: "text/plain".into(),
                        content: "synthetic forbidden effect".into(),
                        size: "synthetic forbidden effect".len() as u64,
                        sha256: hash(b"synthetic forbidden effect"),
                        created_at: clock.highwater_ms,
                    }],
                    ..Default::default()
                })
            })
            .await,
        Err(Error::WorkflowClockRollback)
    ));
    assert_eq!(
        canonical_hash(&before),
        canonical_hash(&store.stored_run(&f.run.id).await.unwrap())
    );
    assert_eq!(
        canonical_hash(&clock),
        canonical_hash(&clock_record(store, &before.record).await)
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM artifacts WHERE run_id=$1")
        .bind(&f.run.id)
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    source.set(clock.highwater_ms);
    store.cancel(&f.run.id).await.unwrap();
    store
        .acknowledge_workflow_cancellation(&f.run.id, &lease)
        .await
        .unwrap();
}
async fn pure_cancel_and_forbidden_delta(store: &Store, source: &ManualUtc) {
    let f = fixture(store).await;
    let parent = started(store, &f).await;
    let original = clock_record(store, &parent.record).await;
    source.set(original.highwater_ms - 1);
    let result = store
        .mutate_with_clock_intent(&f.run.id, None, WorkflowClockIntent::Cancellation, |r| {
            r.transition(RunState::CancelRequested)?;
            r.lease.as_mut().unwrap().until += 1;
            Ok(Change::event(
                "cancel_requested",
                json!({"state":"cancel_requested"}),
            ))
        })
        .await;
    assert!(matches!(result, Err(Error::Forbidden)));
    assert_eq!(
        canonical_hash(&parent),
        canonical_hash(&store.stored_run(&f.run.id).await.unwrap())
    );
    let requested = store.cancel(&f.run.id).await.unwrap();
    assert_eq!(requested.state, RunState::CancelRequested);
    assert!(requested.updated_at >= original.highwater_ms);
    let canceled = store
        .acknowledge_workflow_cancellation(&f.run.id, parent.lease.as_ref().unwrap().token.as_str())
        .await
        .unwrap();
    assert_eq!(canceled.record.state, RunState::Canceled);
    assert!(store.stored_run(&f.run.id).await.unwrap().lease.is_none());
    assert_eq!(canceled.record.started_at, parent.record.started_at);
    assert_eq!(
        canonical_hash(&canceled.record.usage),
        canonical_hash(&parent.record.usage)
    );
    let clock = clock_record(store, &canceled.record).await;
    assert_eq!(clock.highwater_ms, original.highwater_ms);
    assert_eq!(clock.last_run_revision, canceled.record.revision);
    source.set(clock.highwater_ms);
}
async fn missing_and_sticky_queue(store: &Store, source: &ManualUtc) {
    let f = fixture(store).await;
    let original = clock_record(store, &f.run).await;
    sqlx::query("DELETE FROM workflow_execution_clocks WHERE run_id=$1")
        .bind(&f.run.id)
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(matches!(
        execution(store, &f.run).await,
        Err(Error::WorkflowClockUnverified)
    ));
    assert!(matches!(
        store
            .claim_workflow_run(&f.spec.project_id, &f.run.id)
            .await,
        Err(Error::WorkflowClockUnverified)
    ));
    let blocked=store.mutate_with_clock_intent(&f.run.id,None,
        WorkflowClockIntent::QueueNonActivation(WorkflowClockBlock::Unverified),|r| {
        r.record.error_code=Some("workflow_clock_unverified".into());
        Ok(Change::event("workflow_activation_blocked",json!({"state":"queued","error_code":"workflow_clock_unverified","no_action_started":true})))
    }).await.unwrap();
    assert_eq!(blocked.record.state, RunState::Queued);
    assert!(blocked.lease.is_none());
    assert!(blocked.record.started_at.is_none());
    assert!(no_clock(store, &f.run.id).await);
    let independent = fixture(store).await;
    let claim = store.claim_for_worker().await.unwrap().unwrap();
    assert_eq!(
        claim.record.id, independent.run.id,
        "blocked queue entry must not starve another run"
    );
    store.cancel(&independent.run.id).await.unwrap();
    store
        .acknowledge_workflow_cancellation(
            &independent.run.id,
            claim.lease.as_ref().unwrap().token.as_str(),
        )
        .await
        .unwrap();
    let closed = store.cancel(&f.run.id).await.unwrap();
    assert_eq!(closed.state, RunState::Canceled);
    assert!(no_clock(store, &f.run.id).await);
    source.set(original.highwater_ms + 1);

    let g = fixture(store).await;
    let origin = clock_record(store, &g.run).await;
    source.set(origin.highwater_ms - 1);
    let result=store.mutate_with_clock_intent(&g.run.id,None,
        WorkflowClockIntent::QueueNonActivation(WorkflowClockBlock::Rollback),|r| {
        r.record.error_code=Some("workflow_clock_rollback".into());
        Ok(Change::event("workflow_activation_blocked",json!({"state":"queued","error_code":"workflow_clock_rollback","no_action_started":true})))
    }).await.unwrap();
    source.set(origin.highwater_ms + 100);
    assert!(matches!(
        execution(store, &result.record).await,
        Err(Error::WorkflowClockRollback)
    ));
    assert!(matches!(
        store
            .claim_workflow_run(&g.spec.project_id, &g.run.id)
            .await,
        Err(Error::WorkflowClockRollback)
    ));
    assert!(store.stored_run(&g.run.id).await.unwrap().lease.is_none());
    let saved = clock_record(store, &result.record).await;
    let parent_sha = canonical_hash(&store.stored_run(&g.run.id).await.unwrap());
    let mut corrupt = serde_json::to_value(&saved).unwrap();
    corrupt["plan_sha256"] = json!("corrupt-sticky-clock-origin");
    sqlx::query("UPDATE workflow_execution_clocks SET payload=$1 WHERE run_id=$2")
        .bind(json_string(&corrupt).unwrap())
        .bind(&g.run.id)
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(matches!(
        execution(store, &result.record).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        parent_sha,
        canonical_hash(&store.stored_run(&g.run.id).await.unwrap())
    );
    sqlx::query("UPDATE workflow_execution_clocks SET payload=$1 WHERE run_id=$2")
        .bind(json_string(&saved).unwrap())
        .bind(&g.run.id)
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(matches!(
        execution(store, &result.record).await,
        Err(Error::WorkflowClockRollback)
    ));
    store.cancel(&g.run.id).await.unwrap();
}
#[derive(Debug, PartialEq, Eq)]
struct BlockedActivationSnapshot {
    parent_sha256: String,
    workflow_sha256: String,
    events_sha256: String,
    clock_sha256: Option<String>,
}
async fn blocked_activation_snapshot(
    store: &Store,
    f: &Fixture,
    reason: WorkflowClockBlock,
) -> BlockedActivationSnapshot {
    let code = match reason {
        WorkflowClockBlock::Unverified => "workflow_clock_unverified",
        WorkflowClockBlock::Rollback => "workflow_clock_rollback",
    };
    let parent = store.stored_run(&f.run.id).await.unwrap();
    let workflow = store
        .stored_workflow(&f.spec.project_id, &f.run.id)
        .await
        .unwrap();
    let events = store.events(&f.run.id, 0).await.unwrap();
    assert_eq!(parent.record.state, RunState::Queued);
    assert_eq!(parent.record.error_code.as_deref(), Some(code));
    assert!(parent.lease.is_none());
    assert!(parent.record.started_at.is_none());
    assert_eq!(parent.record.steps, 0);
    assert!(parent.record.model_attempts.is_empty());
    assert_eq!(workflow.progress.revision(), 0);
    assert!(serde_json::to_value(&workflow.progress).unwrap()["started_at"].is_null());
    assert!(workflow.progress.attempts().is_empty());
    assert!(workflow.agents.is_empty());
    assert!(workflow.inputs.is_empty());
    assert!(
        workflow
            .progress
            .nodes()
            .values()
            .all(|node| { node.state == WorkflowNodeState::Pending && node.started_at.is_none() })
    );
    assert_eq!(
        events.len(),
        3,
        "Only the two creation events and one block are allowed"
    );
    assert_eq!(parent.record.last_sequence, 3);
    let block = &events[2];
    assert_eq!(block.sequence, 3);
    assert_eq!(block.run_id, f.run.id);
    assert_eq!(block.kind, "workflow_activation_blocked");
    assert_eq!(
        block.payload,
        json!({"state":"queued","error_code":code,"no_new_action_started":true})
    );
    let clock_payload: Option<String> =
        sqlx::query_scalar("SELECT payload FROM workflow_execution_clocks WHERE run_id=$1")
            .bind(&f.run.id)
            .fetch_optional(&store.pool)
            .await
            .unwrap();
    BlockedActivationSnapshot {
        parent_sha256: canonical_hash(&parent),
        workflow_sha256: canonical_hash(&workflow),
        events_sha256: canonical_hash(&events),
        clock_sha256: clock_payload
            .map(|payload| canonical_hash(&parse::<WorkflowClockRecord>(&payload).unwrap())),
    }
}
async fn queue_block_is_once_under_concurrent_scans(store: &Store, source: &ManualUtc) {
    for reason in [WorkflowClockBlock::Unverified, WorkflowClockBlock::Rollback] {
        source.set(now());
        let f = fixture(store).await;
        let origin = clock_record(store, &f.run).await;
        match reason {
            WorkflowClockBlock::Unverified => {
                sqlx::query("DELETE FROM workflow_execution_clocks WHERE run_id=$1")
                    .bind(&f.run.id)
                    .execute(&store.pool)
                    .await
                    .unwrap();
            }
            WorkflowClockBlock::Rollback => source.set(origin.highwater_ms - 1),
        }
        let (a, b, c, d) = tokio::join!(
            Box::pin(store.claim_for_worker()),
            Box::pin(store.claim_for_worker()),
            Box::pin(store.claim_for_worker()),
            Box::pin(store.claim_for_worker())
        );
        for result in [a, b, c, d] {
            assert!(
                result.unwrap().is_none(),
                "Denied work must never acquire a lease"
            );
        }
        let saved = Box::pin(blocked_activation_snapshot(store, &f, reason)).await;
        for _ in 0..4 {
            assert!(store.claim_for_worker().await.unwrap().is_none());
        }
        let replay = store
            .create_workflow_run(&f.request, f.spec.clone())
            .await
            .unwrap();
        assert_eq!(replay.id, f.run.id);
        assert_eq!(replay.created_at, f.run.created_at);
        assert_eq!(canonical_hash(&replay.spec), canonical_hash(&f.run.spec));
        assert_eq!(
            canonical_hash(&replay),
            canonical_hash(&store.stored_run(&f.run.id).await.unwrap().record)
        );
        assert_eq!(
            saved,
            Box::pin(blocked_activation_snapshot(store, &f, reason)).await
        );
        match reason {
            WorkflowClockBlock::Unverified => assert!(no_clock(store, &f.run.id).await),
            WorkflowClockBlock::Rollback => {
                assert_eq!(
                    clock_record(store, &store.stored_run(&f.run.id).await.unwrap().record)
                        .await
                        .highwater_ms,
                    origin.highwater_ms
                );
            }
        }
        // Clock catch-up does not remove the sticky denial, and a blocked row
        // must not starve unrelated admitted work.
        source.set(origin.highwater_ms + 1);
        let independent = fixture(store).await;
        let claim = store.claim_for_worker().await.unwrap().unwrap();
        assert_eq!(claim.record.id, independent.run.id);
        store.cancel(&independent.run.id).await.unwrap();
        store
            .acknowledge_workflow_cancellation(
                &independent.run.id,
                claim.lease.as_ref().unwrap().token.as_str(),
            )
            .await
            .unwrap();
        assert_eq!(
            saved,
            Box::pin(blocked_activation_snapshot(store, &f, reason)).await
        );
        store.cancel(&f.run.id).await.unwrap();
    }
    source.set(now());
}
async fn postgres_queue_block_serialized_replay_is_write_free(store: &Store, source: &ManualUtc) {
    for reason in [WorkflowClockBlock::Unverified, WorkflowClockBlock::Rollback] {
        source.set(now());
        let f = fixture(store).await;
        let origin = clock_record(store, &f.run).await;
        match reason {
            WorkflowClockBlock::Unverified => {
                sqlx::query("DELETE FROM workflow_execution_clocks WHERE run_id=$1")
                    .bind(&f.run.id)
                    .execute(&store.pool)
                    .await
                    .unwrap();
            }
            WorkflowClockBlock::Rollback => source.set(origin.highwater_ms - 1),
        }
        let mut blocker = store.pool.begin().await.unwrap();
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        sqlx::query("UPDATE runs SET revision=revision WHERE id=$1")
            .bind(&f.run.id)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let a_store = store.clone();
        let a_run = f.run.id.clone();
        let b_store = store.clone();
        let b_run = f.run.id.clone();
        let mut a = tokio::spawn(async move {
            Box::pin(a_store.block_workflow_activation(&a_run, reason)).await
        });
        let mut b = tokio::spawn(async move {
            Box::pin(b_store.block_workflow_activation(&b_run, reason)).await
        });
        let waiting_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            // The first real mutation holds the global accounting writer and
            // waits on our run row. The second must wait on that first writer,
            // before reading a run revision. Observe both actual SQL waits;
            // this is serialized replay coverage, not a forced stale-CAS retry.
            let serialized: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pg_stat_activity r JOIN pg_stat_activity a ON r.pid=ANY(pg_blocking_pids(a.pid)) WHERE r.datname=current_database() AND a.datname=current_database() AND $1=ANY(pg_blocking_pids(r.pid)) AND r.query LIKE 'UPDATE runs SET revision=%state=%payload=%updated_at=%' AND a.query='UPDATE accounting_ledgers SET revision=revision WHERE id=$1'",
            ).bind(blocker_pid).fetch_one(&store.pool).await.unwrap();
            if serialized == 1 {
                break;
            }
            if a.is_finished() || b.is_finished() || tokio::time::Instant::now() >= waiting_deadline
            {
                blocker.rollback().await.unwrap();
                let (a_result, b_result) = tokio::join!(&mut a, &mut b);
                panic!(
                    "Owned run/global-writer lock chain was not observed: serialized={serialized}, a={a_result:?}, b={b_result:?}"
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        blocker.rollback().await.unwrap();
        let (a_result, b_result) = tokio::join!(&mut a, &mut b);
        a_result.unwrap().unwrap();
        b_result.unwrap().unwrap();
        let saved = Box::pin(blocked_activation_snapshot(store, &f, reason)).await;
        assert!(store.claim_for_worker().await.unwrap().is_none());
        assert_eq!(
            saved,
            Box::pin(blocked_activation_snapshot(store, &f, reason)).await
        );
        store.cancel(&f.run.id).await.unwrap();
    }
    source.set(now());
}
async fn corrupt_clock_metadata(store: &Store) {
    let f = fixture(store).await;
    let original = clock_record(store, &f.run).await;
    for field in [
        "run_id",
        "project_id",
        "order_id",
        "spec_sha256",
        "plan_sha256",
        "created_at_ms",
        "last_run_revision",
        "creation_confirmed",
    ] {
        let mut payload = serde_json::to_value(&original).unwrap();
        payload[field] = match field {
            "created_at_ms" | "last_run_revision" => json!(-1),
            "creation_confirmed" => json!(false),
            _ => json!("different-scope"),
        };
        sqlx::query("UPDATE workflow_execution_clocks SET payload=$1 WHERE run_id=$2")
            .bind(json_string(&payload).unwrap())
            .bind(&f.run.id)
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(
            matches!(execution(store, &f.run).await, Err(Error::Forbidden)),
            "{field}"
        );
    }
    sqlx::query("UPDATE workflow_execution_clocks SET payload=$1 WHERE run_id=$2")
        .bind(json_string(&original).unwrap())
        .bind(&f.run.id)
        .execute(&store.pool)
        .await
        .unwrap();
    let saved: String =
        sqlx::query_scalar("SELECT payload FROM events WHERE run_id=$1 AND sequence=1")
            .bind(&f.run.id)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    sqlx::query("UPDATE events SET payload='{}' WHERE run_id=$1 AND sequence=1")
        .bind(&f.run.id)
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(matches!(
        execution(store, &f.run).await,
        Err(Error::Forbidden)
    ));
    sqlx::query("UPDATE events SET payload=$1 WHERE run_id=$2 AND sequence=1")
        .bind(saved)
        .bind(&f.run.id)
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(execution(store, &f.run).await.is_ok());
    store.cancel(&f.run.id).await.unwrap();
}
async fn unknown_admission_cleanup_preserves_unconfirmed_effects(
    store: &Store,
    source: &ManualUtc,
) {
    for cancel in [false, true] {
        source.set(now());
        let f = fixture(store).await;
        let parent = started(store, &f).await;
        let lease = parent.lease.as_ref().unwrap().token.clone();
        let claim = match store
            .workflow_command(
                &f.spec.project_id,
                &f.run.id,
                &lease,
                0,
                workflows::WorkflowCommand::Claim {
                    node_id: "a".into(),
                },
            )
            .await
            .unwrap()
        {
            workflows::WorkflowCommandResult::Claimed(claim) => claim,
            _ => panic!("actual node claim required"),
        };
        let (revision, context) = store
            .workflow_model_context(&f.spec.project_id, &f.run.id, &claim, json!([]))
            .await
            .unwrap();
        let admission = store
            .begin_workflow_model(
                &f.spec.project_id,
                &f.run.id,
                &lease,
                revision,
                &claim,
                "unknown-clock-fixture",
                &context,
            )
            .await
            .unwrap();
        assert!(matches!(
            admission,
            workflows::agents::WorkflowModelAdmission::Dispatch(_)
        ));
        let before = store.stored_run(&f.run.id).await.unwrap();
        let detail = store.detail(&f.run.id).await.unwrap();
        assert_eq!(before.record.model_attempts.len(), 1);
        let attempt = &before.record.model_attempts[0];
        assert_eq!(attempt.state, ModelAttemptState::InFlight);
        assert!(attempt.finished_at.is_none());
        assert!(detail.token_budget.unconfirmed_tokens > 0);
        let original_clock = clock_record(store, &before.record).await;
        source.set(original_clock.highwater_ms - 1);
        let after = if cancel {
            let requested = store.cancel(&f.run.id).await.unwrap();
            assert_eq!(requested.state, RunState::CancelRequested);
            store
                .acknowledge_workflow_cancellation(&f.run.id, &lease)
                .await
                .unwrap()
        } else {
            store
                .interrupt_workflow_run(&f.run.id, &lease, "workflow_clock_rollback")
                .await
                .unwrap()
        };
        assert_eq!(after.record.state, RunState::Interrupted);
        assert!(after.lease.is_none());
        assert_eq!(after.record.steps, before.record.steps);
        assert_eq!(after.record.started_at, before.record.started_at);
        assert_eq!(after.record.model_attempts.len(), 1);
        let preserved = &after.record.model_attempts[0];
        assert_eq!(preserved.state, ModelAttemptState::Interrupted);
        assert!(preserved.finished_at.is_none());
        let mut normalized = preserved.clone();
        normalized.state = attempt.state.clone();
        assert_eq!(canonical_hash(&normalized), canonical_hash(attempt));
        assert_eq!(
            canonical_hash(&after.record.usage),
            canonical_hash(&before.record.usage)
        );
        let final_detail = store.detail(&f.run.id).await.unwrap();
        assert_eq!(
            canonical_hash(&final_detail.token_budget),
            canonical_hash(&detail.token_budget)
        );
        assert!(final_detail.artifacts.is_empty());
        let workflow = store
            .stored_workflow(&f.spec.project_id, &f.run.id)
            .await
            .unwrap();
        let domain_attempt = &workflow.progress.attempts()["unknown-clock-fixture"];
        assert!(domain_attempt.outcome.is_none());
        assert!(domain_attempt.finished_at.is_none());
        let turn = store
            .workflow_model_turn(&f.spec.project_id, &f.run.id, "unknown-clock-fixture")
            .await
            .unwrap()
            .unwrap();
        assert!(turn.response.is_none());
        let final_clock = clock_record(store, &after.record).await;
        assert_eq!(final_clock.highwater_ms, original_clock.highwater_ms);
        assert_eq!(final_clock.last_run_revision, after.record.revision);

        // Direct guard negatives are separate from the actual admission/cleanup above.
        let mut forged = preserved.clone();
        forged.reserved_tokens = forged.reserved_tokens.saturating_sub(1);
        assert!(!conservative_models(
            &before.record.model_attempts,
            &[forged],
            after.record.updated_at
        ));
        let mut forged = preserved.clone();
        forged.state = ModelAttemptState::Measured;
        assert!(!conservative_models(
            &before.record.model_attempts,
            &[forged],
            after.record.updated_at
        ));
        let mut forged = preserved.clone();
        forged.finished_at = Some(attempt.started_at - 1);
        assert!(!conservative_models(
            &before.record.model_attempts,
            &[forged],
            after.record.updated_at
        ));
    }
}
async fn contract(store: &Store, source: &ManualUtc) {
    Box::pin(seed_replay_and_single_agent(store, source)).await;
    Box::pin(rollback_and_concurrent_mutations(store, source)).await;
    Box::pin(pure_cancel_and_forbidden_delta(store, source)).await;
    Box::pin(missing_and_sticky_queue(store, source)).await;
    Box::pin(queue_block_is_once_under_concurrent_scans(store, source)).await;
    Box::pin(corrupt_clock_metadata(store)).await;
    Box::pin(unknown_admission_cleanup_preserves_unconfirmed_effects(
        store, source,
    ))
    .await;
}
#[tokio::test]
async fn sqlite_workflow_clock_contract() {
    let mut store = Store::connect("sqlite::memory:").await.unwrap();
    let source = install(&mut store);
    Box::pin(contract(&store, &source)).await;
    store.close().await;
}
#[tokio::test]
async fn sqlite_clock_reopen_and_missing_record_has_no_legacy_seed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clock.sqlite3");
    let mut store = Store::sqlite(&path).await.unwrap();
    let source = install(&mut store);
    let f = fixture(&store).await;
    let original = clock_record(&store, &f.run).await;
    store.close().await;
    let reopened = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        canonical_hash(&original),
        canonical_hash(&clock_record(&reopened, &f.run).await)
    );
    // Keep the current schema and every original migration receipt coherent.
    // A missing clock is a failclosed recovery case, not a fabricated v19 DB.
    let migration_receipts: Vec<(i64, String)> =
        sqlx::query_as("SELECT version,checksum FROM schema_migrations ORDER BY version")
            .fetch_all(&reopened.pool)
            .await
            .unwrap();
    assert_eq!(
        migration_receipts.len(),
        crate::migrations::MIGRATIONS.len()
    );
    assert_eq!(
        sqlx::query("DELETE FROM workflow_execution_clocks WHERE run_id=$1")
            .bind(&f.run.id)
            .execute(&reopened.pool)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
    reopened.close().await;
    let mut upgraded = Store::sqlite(&path).await.unwrap();
    upgraded.workflow_clock = WorkflowClock::for_test(source.clone());
    assert_eq!(
        sqlx::query_as::<_, (i64, String)>(
            "SELECT version,checksum FROM schema_migrations ORDER BY version"
        )
        .fetch_all(&upgraded.pool)
        .await
        .unwrap(),
        migration_receipts
    );
    assert!(no_clock(&upgraded, &f.run.id).await);
    assert!(matches!(
        execution(&upgraded, &f.run).await,
        Err(Error::WorkflowClockUnverified)
    ));
    let canceled = upgraded.cancel(&f.run.id).await.unwrap();
    assert_eq!(canceled.state, RunState::Canceled);
    assert!(no_clock(&upgraded, &f.run.id).await);
    assert_eq!(
        upgraded.events(&f.run.id, 0).await.unwrap()[0].created_at,
        f.run.created_at
    );
    upgraded.close().await;
}
// Genuine V2 schema19 -> current migration, with a synthetic retained SingleRun.
// The payload uses current DTOs with optional/default fields explicitly empty;
// this tests schema migration and byte preservation, not historical app parsing.
#[tokio::test]
async fn sqlite_schema19_upgrade_preserves_retained_single_run_without_clock_seed() {
    use sqlx::Connection;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("actual-schema19.sqlite3");
    let mut connection = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    let mut tx = connection.begin().await.unwrap();
    let mut original_receipts = Vec::new();
    // At this point all data tables are empty, so the historical data backfills
    // for migrations2/9/17 have no rows to transform. Insert retained data only
    // after the complete original1..19 schema and receipts exist.
    for (version, sql) in crate::migrations::MIGRATIONS
        .iter()
        .take_while(|(v, _)| *v <= 19)
    {
        for statement in sql.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            sqlx::query(statement).execute(&mut *tx).await.unwrap();
        }
        let checksum = hash(sql.as_bytes());
        sqlx::query("INSERT INTO schema_migrations(version,checksum) VALUES($1,$2)")
            .bind(*version)
            .bind(&checksum)
            .execute(&mut *tx)
            .await
            .unwrap();
        original_receipts.push((*version, checksum));
    }
    assert_eq!(original_receipts.len(), 19);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('workflow_execution_clocks','productivity_guard')"
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap(),
        0
    );
    let created_at = now();
    let project = Project {
        id: id(),
        name: "Synthetic retained schema19 project".into(),
        revision: 1,
        created_at,
    };
    let order = Order {
        id: id(),
        project_id: project.id.clone(),
        title: "Retained single run".into(),
        draft: "Retain this original draft byte for byte.\u{00e4}".into(),
        revision: 1,
        created_at,
    };
    let provider = ProviderProfile {
        generation: Default::default(),
        id: id(),
        name: "Synthetic retained profile".into(),
        kind: ProviderKind::Test,
        endpoint: String::new(),
        model: "schema19-fixture".into(),
        revision: 1,
        context_tokens: 2048,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    let spec = RunSpec {
        input_workspace: None,
        agent_config: None,
        model_binding: None,
        output: None,
        workflow: None,
        mcp_tools: vec![],
        environment: None,
        folders: vec![],
        order_id: order.id.clone(),
        project_id: project.id.clone(),
        prompt: "Original synthetic pre-clock single request".into(),
        provider: Some(provider.clone()),
        file_action: None,
        access_mode: AccessMode::ConfirmMutations,
        executor: "local-v2".into(),
        policy_revision: 1,
        allowed_tools: vec![],
        budgets: Budgets::default(),
    };
    let retained = StoredRun {
        record: RunRecord {
            model_attempts: vec![],
            id: id(),
            spec,
            state: RunState::Queued,
            revision: 1,
            last_sequence: 1,
            steps: 0,
            created_at,
            updated_at: created_at,
            started_at: None,
            error_code: None,
            usage: Usage::default(),
        },
        checkpoint: Checkpoint {
            messages: vec![json!({"role":"user","content":"Original retained single-run seed"})],
            ..Default::default()
        },
        lease: None,
    };
    let event = RunEvent {
        schema_version: SCHEMA_VERSION,
        id: id(),
        run_id: retained.record.id.clone(),
        sequence: 1,
        kind: "run_queued".into(),
        payload: json!({"state":"queued"}),
        created_at,
    };
    let project_bytes = json_string(&project).unwrap();
    let order_bytes = json_string(&order).unwrap();
    let provider_bytes = json_string(&provider).unwrap();
    let run_bytes = json_string(&retained).unwrap();
    let event_bytes = json_string(&event).unwrap();
    sqlx::query("INSERT INTO projects(id,payload) VALUES($1,$2)")
        .bind(&project.id)
        .bind(&project_bytes)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO orders(id,project_id,revision,payload) VALUES($1,$2,$3,$4)")
        .bind(&order.id)
        .bind(&project.id)
        .bind(order.revision)
        .bind(&order_bytes)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO providers(id,payload) VALUES($1,$2)")
        .bind(&provider.id)
        .bind(&provider_bytes)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO runs(id,order_id,idempotency_key,request_hash,revision,state,payload,updated_at,execution_kind) VALUES($1,$2,$3,$4,1,'queued',$5,$6,'agent')")
        .bind(&retained.record.id).bind(&order.id).bind("original-schema19-single")
        .bind(hash(b"synthetic-schema19-request"))
        .bind(&run_bytes).bind(created_at).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO events(id,run_id,sequence,payload) VALUES($1,$2,1,$3)")
        .bind(&event.id)
        .bind(&retained.record.id)
        .bind(&event_bytes)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_as::<_, (i64, String)>(
            "SELECT version,checksum FROM schema_migrations ORDER BY version"
        )
        .fetch_all(&mut *tx)
        .await
        .unwrap(),
        original_receipts
    );
    tx.commit().await.unwrap();
    connection.close().await.unwrap();

    // This is the first Store open of this real schema19 DB; only production
    // Store migration applies20..current, including all real backfill hooks.
    let upgraded = Store::sqlite(&path).await.unwrap();
    let receipts: Vec<(i64, String)> =
        sqlx::query_as("SELECT version,checksum FROM schema_migrations ORDER BY version")
            .fetch_all(&upgraded.pool)
            .await
            .unwrap();
    let expected: Vec<_> = crate::migrations::MIGRATIONS
        .iter()
        .map(|(version, sql)| (*version, hash(sql.as_bytes())))
        .collect();
    assert_eq!(receipts, expected);
    assert_eq!(&receipts[..19], original_receipts.as_slice());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM productivity_guard WHERE id=1")
            .fetch_one(&upgraded.pool)
            .await
            .unwrap(),
        1
    );
    for (table, key, original) in [
        ("projects", project.id.as_str(), project_bytes.as_str()),
        ("orders", order.id.as_str(), order_bytes.as_str()),
        ("providers", provider.id.as_str(), provider_bytes.as_str()),
        ("runs", retained.record.id.as_str(), run_bytes.as_str()),
        ("events", event.id.as_str(), event_bytes.as_str()),
    ] {
        let sql = format!("SELECT payload FROM {table} WHERE id=$1");
        let actual: String = sqlx::query_scalar(&sql)
            .bind(key)
            .fetch_one(&upgraded.pool)
            .await
            .unwrap();
        assert_eq!(
            actual.as_bytes(),
            original.as_bytes(),
            "{table} original payload changed"
        );
    }
    let restored = upgraded.stored_run(&retained.record.id).await.unwrap();
    assert_eq!(canonical_hash(&restored), canonical_hash(&retained));
    assert!(restored.record.spec.workflow.is_none());
    assert!(restored.lease.is_none());
    assert!(no_clock(&upgraded, &retained.record.id).await);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_execution_clocks")
            .fetch_one(&upgraded.pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        upgraded.events(&retained.record.id, 0).await.unwrap()[0].created_at,
        created_at
    );
    upgraded.close().await;
}
#[tokio::test]
async fn sqlite_creation_final_clock_fence_rolls_back_all_rows() {
    let mut store = Store::connect("sqlite::memory:").await.unwrap();
    let source = install(&mut store);
    let f = fixture(&store).await;
    let existing = store.stored_run(&f.run.id).await.unwrap();
    let wf = store
        .stored_workflow(&f.spec.project_id, &f.run.id)
        .await
        .unwrap();
    let mut new = existing.clone();
    new.record.id = id();
    let mut newwf = wf.clone();
    newwf.run_id = new.record.id.clone();
    let mut tx = store.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO runs(id,order_id,idempotency_key,request_hash,revision,state,payload,updated_at,execution_kind) VALUES($1,$2,$3,$4,1,'queued',$5,$6,'agent')")
        .bind(&new.record.id).bind(&new.record.spec.order_id).bind(id()).bind("synthetic")
        .bind(json_string(&new).unwrap()).bind(new.record.updated_at).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO workflow_runs(run_id,workflow_id,version_id,project_id,revision,plan_sha256,progress_sha256,payload) VALUES($1,$2,$3,$4,0,$5,$6,$7)")
        .bind(&newwf.run_id).bind(&newwf.binding.workflow_id).bind(&newwf.binding.version_id).bind(&newwf.project_id)
        .bind(&newwf.plan.sha256).bind(canonical_hash(&newwf.progress)).bind(json_string(&newwf).unwrap())
        .execute(&mut *tx).await.unwrap();
    store
        .workflow_clock
        .seed(&mut tx, &new.record, &newwf)
        .await
        .unwrap();
    for (sequence, kind, payload) in [
        (1, "run_queued", json!({"state":"queued"})),
        (
            2,
            "workflow_initialized",
            json!({"binding":new.record.spec.workflow}),
        ),
    ] {
        insert_event(
            &mut tx,
            &RunEvent {
                schema_version: SCHEMA_VERSION,
                id: id(),
                run_id: new.record.id.clone(),
                sequence,
                kind: kind.into(),
                payload,
                created_at: new.record.created_at,
            },
        )
        .await
        .unwrap();
    }
    source.set(new.record.created_at - 1);
    assert!(matches!(
        store
            .workflow_clock
            .finish_creation(&mut tx, &new.record)
            .await,
        Err(Error::WorkflowClockRollback)
    ));
    tx.rollback().await.unwrap();
    assert!(matches!(
        store.stored_run(&new.record.id).await,
        Err(Error::NotFound)
    ));
    assert!(no_clock(&store, &new.record.id).await);
    let workflows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_runs WHERE run_id=$1")
        .bind(&new.record.id)
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(workflows, 0);
    store.close().await;
}
#[tokio::test]
#[ignore = "Requires a dedicated COWORK_TEST_POSTGRES database"]
async fn postgres_workflow_clock_contract() {
    let url = std::env::var("COWORK_TEST_POSTGRES").unwrap();
    let mut store = Store::connect(&url).await.unwrap();
    let source = install(&mut store);
    Box::pin(contract(&store, &source)).await;
    Box::pin(postgres_queue_block_serialized_replay_is_write_free(
        &store, &source,
    ))
    .await;
    Box::pin(postgres_clock_lock_wait(&store, &source, false)).await;
    Box::pin(postgres_clock_lock_wait(&store, &source, true)).await;
    store.close().await;
}
async fn postgres_clock_lock_wait(store: &Store, source: &Arc<ManualUtc>, expire_lease: bool) {
    source.set(now());
    let f = fixture(store).await;
    let parent = started(store, &f).await;
    let lease = parent.lease.as_ref().unwrap().clone();
    let original = clock_record(store, &parent.record).await;
    let mut blocker = store.pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    sqlx::query("UPDATE workflow_execution_clocks SET revision=revision WHERE run_id=$1")
        .bind(&f.run.id)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let other = store.clone();
    let run = f.run.id.clone();
    let token = lease.token.clone();
    let task = tokio::spawn(async move { other.heartbeat(&run, &token).await });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let waiting:i64=sqlx::query_scalar("SELECT COUNT(*) FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%workflow_execution_clocks%'")
            .bind(pid).fetch_one(&store.pool).await.unwrap();
        if waiting > 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "actual Clockrow SQL lock wait not observed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    source.set(if expire_lease {
        lease.until + 1
    } else {
        original.highwater_ms - 1
    });
    blocker.commit().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    if expire_lease {
        assert!(matches!(result, Err(Error::StaleLease)));
    } else {
        assert!(matches!(result, Err(Error::WorkflowClockRollback)));
    }
    assert_eq!(
        canonical_hash(&parent),
        canonical_hash(&store.stored_run(&f.run.id).await.unwrap())
    );
    assert_eq!(
        canonical_hash(&original),
        canonical_hash(&clock_record(store, &parent.record).await)
    );
}

struct ScriptedUtc {
    observations: std::sync::Mutex<Vec<i64>>,
    values: std::sync::Mutex<std::collections::VecDeque<i64>>,
    last: AtomicI64,
}
impl ScriptedUtc {
    fn new(values: impl IntoIterator<Item = i64>) -> Arc<Self> {
        Arc::new(Self {
            observations: std::sync::Mutex::new(vec![]),
            values: std::sync::Mutex::new(values.into_iter().collect()),
            last: AtomicI64::new(0),
        })
    }
}
impl UtcSource for ScriptedUtc {
    fn now(&self) -> i64 {
        if let Some(value) = self.values.lock().unwrap().pop_front() {
            self.last.store(value, Ordering::SeqCst);
        }
        let sample = self.last.load(Ordering::SeqCst);
        self.observations.lock().unwrap().push(sample);
        sample
    }
}
#[tokio::test]
async fn sqlite_execution_tx_rejects_own_stage_and_final_probe_rollback() {
    let mut store = Store::connect("sqlite::memory:").await.unwrap();
    let source = install(&mut store);
    let f = fixture(&store).await;
    let before = started(&store, &f).await;
    let original = clock_record(&store, &before.record).await;
    for (final_offset, caller_floor, post_write_regression) in
        [(90, 0, false), (150, 200, false), (300, 0, true)]
    {
        source.set(original.highwater_ms + 100);
        let mut tx = store.pool.begin().await.unwrap();
        let stage = store
            .workflow_clock
            .stage(&mut tx, &before, WorkflowClockIntent::Execution)
            .await
            .unwrap();
        assert_eq!(
            stage.execution_floor_ms(),
            Some(original.highwater_ms + 100)
        );
        let mut after = before.clone();
        after.record.revision += 1;
        after.record.last_sequence += 1;
        after.record.updated_at = original.highwater_ms + 100;
        assert_eq!(sqlx::query("UPDATE runs SET revision=$1,state=$2,updated_at=$3,payload=$4 WHERE id=$5 AND revision=$6")
            .bind(after.record.revision).bind(state_name(&after.record.state)).bind(after.record.updated_at)
            .bind(json_string(&after).unwrap()).bind(&after.record.id).bind(before.record.revision)
            .execute(&mut *tx).await.unwrap().rows_affected(),1);
        let payload = json!({"synthetic_sql_effect":true});
        insert_event(
            &mut tx,
            &RunEvent {
                schema_version: SCHEMA_VERSION,
                id: id(),
                run_id: after.record.id.clone(),
                sequence: after.record.last_sequence,
                kind: "clock_fixture_effect".into(),
                payload: payload.clone(),
                created_at: after.record.updated_at,
            },
        )
        .await
        .unwrap();
        let artifact = Artifact {
            id: id(),
            run_id: after.record.id.clone(),
            order_id: after.record.spec.order_id.clone(),
            name: "uncommitted synthetic effect".into(),
            media_type: "text/plain".into(),
            size: 4,
            sha256: hash(b"test"),
            content: "test".into(),
            created_at: after.record.updated_at,
        };
        sqlx::query("INSERT INTO artifacts(id,run_id,order_id,payload) VALUES($1,$2,$3,$4)")
            .bind(&artifact.id)
            .bind(&artifact.run_id)
            .bind(&artifact.order_id)
            .bind(json_string(&artifact).unwrap())
            .execute(&mut *tx)
            .await
            .unwrap();
        let change = Change {
            events: vec![("clock_fixture_effect".into(), payload)],
            artifacts: vec![artifact],
            ..Default::default()
        };
        let clock = if post_write_regression {
            WorkflowClock::for_test(ScriptedUtc::new([
                original.highwater_ms + 300,
                original.highwater_ms + 299,
            ]))
        } else {
            source.set(original.highwater_ms + final_offset);
            store.workflow_clock.clone()
        };
        assert!(matches!(
            clock
                .finish(
                    &mut tx,
                    &stage,
                    &after,
                    &change,
                    original.highwater_ms + caller_floor
                )
                .await,
            Err(Error::WorkflowClockRollback)
        ));
        tx.rollback().await.unwrap();
        assert_eq!(
            canonical_hash(&before),
            canonical_hash(&store.stored_run(&f.run.id).await.unwrap())
        );
        assert_eq!(
            canonical_hash(&original),
            canonical_hash(&clock_record(&store, &before.record).await)
        );
        let effects: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM artifacts WHERE run_id=$1")
            .bind(&f.run.id)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(effects, 0);
        assert_eq!(
            store.events(&f.run.id, 0).await.unwrap().len() as i64,
            before.record.last_sequence
        );
    }
    store.close().await;
}

#[tokio::test]
async fn sqlite_actual_mutate_final_guard_cannot_use_an_older_probe() {
    let mut store = Store::connect("sqlite::memory:").await.unwrap();
    install(&mut store);
    Box::pin(actual_mutate_final_guard(&store)).await;
    store.close().await;
}
async fn actual_mutate_final_guard(original_store: &Store) {
    let mut store = original_store.clone();
    let f = fixture(&store).await;
    let before = store.stored_run(&f.run.id).await.unwrap();
    let original = clock_record(&store, &before.record).await;
    let h = original.highwater_ms;
    let values = [h, h, h, h + 2, h + 2, h + 1];
    let source = ScriptedUtc::new(values);
    store.workflow_clock = WorkflowClock::for_test(source.clone());
    let result = store
        .mutate(&f.run.id, None, |_| {
            Ok(Change {
                events: vec![("clock_fixture_effect".into(), json!({"synthetic":true}))],
                artifacts: vec![Artifact {
                    id: id(),
                    run_id: f.run.id.clone(),
                    order_id: f.spec.order_id.clone(),
                    name: "final guard synthetic result".into(),
                    media_type: "text/plain".into(),
                    size: 4,
                    sha256: hash(b"test"),
                    content: "test".into(),
                    created_at: h,
                }],
                ..Default::default()
            })
        })
        .await;
    assert!(matches!(result, Err(Error::WorkflowClockRollback)));
    assert_eq!(
        source.observations.lock().unwrap().as_slice(),
        values,
        "actual own Store phase samples must show final probe behind persisted HWM"
    );
    assert_eq!(
        canonical_hash(&before),
        canonical_hash(&store.stored_run(&f.run.id).await.unwrap())
    );
    assert_eq!(
        canonical_hash(&original),
        canonical_hash(&clock_record(&store, &before.record).await)
    );
    assert_eq!(
        store.events(&f.run.id, 0).await.unwrap().len() as i64,
        before.record.last_sequence
    );
    let artifacts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM artifacts WHERE run_id=$1")
        .bind(&f.run.id)
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(artifacts, 0);
}
