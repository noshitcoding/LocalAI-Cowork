use cowork_contracts::*;
use cowork_domain::{canonical_hash, workflows::*};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use cowork_workflow_contracts::{CommitExecutionPlan, RunRequest, WorkflowRunRequest};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

struct Fixture {
    temp: tempfile::TempDir,
    runtime: Runtime,
    project: Project,
    request: WorkflowRunRequest,
}
impl Fixture {
    async fn new(delay: u32) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&temp.path().join("waits.sqlite3"))
            .await
            .unwrap();
        Self::with_store(delay, temp, store).await
    }
    async fn with_store(delay: u32, temp: tempfile::TempDir, store: Store) -> Self {
        let project = store
            .create_project(CreateProject {
                name: "Wait qualification".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Durable wait".into(),
            })
            .await
            .unwrap();
        let definition: WorkflowDefinition =
            serde_json::from_str(include_str!("../../../tests/fixtures/workflow-wait.json"))
                .unwrap();
        let entry = store
            .create_workflow(&project.id, "wait-definition", &definition)
            .await
            .unwrap();
        let request=serde_json::from_value(json!({"kind":"workflow","order_id":order.id,"base_revision":order.revision,"idempotency_key":"wait-run","prompt":"Run the published wait.","workflow_id":entry.id,"version_id":entry.versions[0].id,"access_mode":"read_only","budgets":definition.budgets,"inputs":{"delay":delay}})).unwrap();
        let runtime = Runtime::new(store, vec![], false)
            .unwrap()
            .with_file_broker(
                cowork_broker::FileBroker::open(&temp.path().join("broker"), vec![]).unwrap(),
            );
        Self {
            temp,
            runtime,
            project,
            request,
        }
    }
    async fn create(&self) -> RunRecord {
        let prepared = self
            .runtime
            .prepare_execution_plan(&RunRequest::Workflow(self.request.clone()))
            .await
            .unwrap();
        self.runtime
            .commit_execution_plan(
                &self.request.order_id,
                &prepared.plan.id,
                &CommitExecutionPlan {
                    sha256: prepared.sha256,
                },
            )
            .await
            .unwrap()
    }
    async fn park(&self) -> RunRecord {
        let run = self.create().await;
        let waiting = self
            .runtime
            .run_native_workflow(&self.project.id, &run.id, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(waiting.state, RunState::WaitingForTimer, "{waiting:?}");
        assert!(
            self.runtime
                .store
                .stored_run(&run.id)
                .await
                .unwrap()
                .lease
                .is_none()
        );
        waiting
    }
    async fn interrupt_before_parking(&self) -> RunRecord {
        use cowork_storage::{WorkflowCommand, WorkflowCommandResult};
        let run = self.create().await;
        let parent = self
            .runtime
            .store
            .claim_workflow_run(&self.project.id, &run.id)
            .await
            .unwrap();
        let lease = parent.lease.as_ref().unwrap().token.clone();
        let WorkflowCommandResult::Claimed(claim) = self
            .runtime
            .store
            .workflow_command(
                &self.project.id,
                &run.id,
                &lease,
                0,
                WorkflowCommand::Claim {
                    node_id: "wait".into(),
                },
            )
            .await
            .unwrap()
        else {
            panic!("native claim")
        };
        let graph = self
            .runtime
            .store
            .stored_workflow(&self.project.id, &run.id)
            .await
            .unwrap();
        self.runtime
            .store
            .workflow_command(
                &self.project.id,
                &run.id,
                &lease,
                graph.progress.revision(),
                WorkflowCommand::Control {
                    claim,
                    id: "durable-before-park".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            self.runtime
                .store
                .stored_run(&run.id)
                .await
                .unwrap()
                .record
                .state,
            RunState::Running
        );
        self.runtime
            .store
            .interrupt_workflow_run(&run.id, &lease, "fixture_restart_before_park")
            .await
            .unwrap()
            .record
    }
    async fn pool(&self) -> sqlx::SqlitePool {
        sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(self.temp.path().join("waits.sqlite3")),
        )
        .await
        .unwrap()
    }
    async fn index_count(&self, run: &str) -> i64 {
        let db = self.pool().await;
        let n = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_timer_wakes WHERE run_id=$1")
            .bind(run)
            .fetch_one(&db)
            .await
            .unwrap();
        db.close().await;
        n
    }
    async fn due(&self, run: &str) {
        let graph = self
            .runtime
            .store
            .stored_workflow(&self.project.id, run)
            .await
            .unwrap();
        let wake = graph.progress.next_wake_at().unwrap() * 1000;
        let remaining = wake.saturating_sub(cowork_storage::now());
        if remaining > 0 {
            tokio::time::sleep(Duration::from_millis(remaining as u64 + 40)).await;
        }
    }
    async fn fast_run(&self) -> RunRecord {
        let order = self
            .runtime
            .store
            .create_order(CreateOrder {
                project_id: self.project.id.clone(),
                title: "Independent fast work".into(),
            })
            .await
            .unwrap();
        let mut d: WorkflowDefinition =
            serde_json::from_str(include_str!("../../../tests/fixtures/workflow-wait.json"))
                .unwrap();
        d.name = "Fast work".into();
        d.controls.remove("wait");
        d.controls.get_mut("after").unwrap().dependencies.clear();
        let data = d.dataflow.as_mut().unwrap();
        data.input = WorkflowDataType::Object {
            fields: Default::default(),
        };
        data.steps.remove("wait");
        data.steps
            .get_mut("after")
            .unwrap()
            .inputs
            .get_mut("scheduled_at")
            .unwrap()
            .value = WorkflowDataSource::Literal { value: json!(7) };
        let entry = self
            .runtime
            .store
            .create_workflow(&self.project.id, "fast-definition", &d)
            .await
            .unwrap();
        let input:WorkflowRunRequest=serde_json::from_value(json!({"kind":"workflow","order_id":order.id,"base_revision":order.revision,"idempotency_key":"fast-run","prompt":"Finish independent work.","workflow_id":entry.id,"version_id":entry.versions[0].id,"access_mode":"read_only","budgets":d.budgets,"inputs":{}})).unwrap();
        let plan = self
            .runtime
            .prepare_execution_plan(&RunRequest::Workflow(input))
            .await
            .unwrap();
        self.runtime
            .commit_execution_plan(
                &order.id,
                &plan.plan.id,
                &CommitExecutionPlan {
                    sha256: plan.sha256,
                },
            )
            .await
            .unwrap()
    }
}
async fn state(store: &Store, id: &str, expected: RunState) -> RunRecord {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let run = store.stored_run(id).await.unwrap().record;
            if run.state == expected {
                return run;
            }
            assert!(
                !matches!(
                    run.state,
                    RunState::Failed | RunState::Interrupted | RunState::Expired
                ),
                "{run:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn timer_does_not_complete_early_and_resumes_the_same_run_without_tokens() {
    let f = Fixture::new(3).await;
    let waiting = f.park().await;
    let activity = f.runtime.store.activity(false, 0, 20).await.unwrap();
    assert_eq!(activity.total, 1);
    assert_eq!(activity.attention, 0);
    assert_eq!(activity.items[0].run.id, waiting.id);
    assert_eq!(f.index_count(&waiting.id).await, 1);
    assert_eq!(f.runtime.store.wake_workflow_timers().await.unwrap(), 0);
    let before = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &waiting.id)
        .await
        .unwrap();
    let at = before.progress.next_wake_at();
    assert_eq!(
        serde_json::to_value(&before.progress).unwrap()["format_version"],
        9
    );
    assert!(at.unwrap() * 1000 >= waiting.started_at.unwrap() + 3000);
    let view = f
        .runtime
        .store
        .workflow_run_view(&f.project.id, &waiting.id)
        .await
        .unwrap();
    assert_eq!(
        view.state,
        cowork_workflow_contracts::WorkflowPublicState::WaitingTimer
    );
    assert!(view.nodes.iter().any(|n| n.wake_at == at));
    f.due(&waiting.id).await;
    assert_eq!(f.runtime.store.wake_workflow_timers().await.unwrap(), 1);
    let result = f
        .runtime
        .run_native_workflow(&f.project.id, &waiting.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.state, RunState::Completed);
    assert_eq!(
        f.runtime.store.activity(false, 0, 20).await.unwrap().total,
        0
    );
    assert_eq!(result.started_at, waiting.started_at);
    assert!(result.model_attempts.is_empty());
    assert_eq!(f.index_count(&waiting.id).await, 0);
    let after = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &waiting.id)
        .await
        .unwrap();
    assert_eq!(after.progress.attempts().len(), 2);
    assert_eq!(after.progress.charged_tokens(), 0);
    let original = before
        .progress
        .attempts()
        .values()
        .find(|a| a.claim.node_id == "wait")
        .unwrap();
    assert_eq!(
        original.claim,
        after.progress.attempts()[&original.id].claim
    );
}
#[tokio::test]
async fn one_worker_processes_another_run_while_the_first_is_waiting() {
    let f = Fixture::new(10).await;
    let run = f.create().await;
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f.runtime.store, &run.id, RunState::WaitingForTimer).await;
    let wake = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap()
        .progress
        .next_wake_at()
        .unwrap()
        * 1000;
    let fast = f.fast_run().await;
    let finished = state(&f.runtime.store, &fast.id, RunState::Completed).await;
    assert!(
        finished.updated_at < wake,
        "The sole worker must finish independent work before the timer is due"
    );
    state(&f.runtime.store, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
}
#[tokio::test]
async fn reopen_preserves_the_original_wake_time_and_automatic_continuation() {
    let f = Fixture::new(3).await;
    let run = f.park().await;
    let original = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let wake = original.progress.next_wake_at();
    f.runtime.store.close().await;
    let reopened = Store::sqlite(&f.temp.path().join("waits.sqlite3"))
        .await
        .unwrap();
    assert_eq!(
        reopened
            .stored_workflow(&f.project.id, &run.id)
            .await
            .unwrap()
            .progress
            .next_wake_at(),
        wake
    );
    let runtime = Runtime::new(reopened, vec![], false)
        .unwrap()
        .with_file_broker(
            cowork_broker::FileBroker::open(&f.temp.path().join("broker"), vec![]).unwrap(),
        );
    let stop = CancellationToken::new();
    let worker = tokio::spawn(runtime.clone().serve(1, stop.clone()));
    let complete = state(&runtime.store, &run.id, RunState::Completed).await;
    assert_eq!(complete.started_at, run.started_at);
    stop.cancel();
    worker.await.unwrap();
    let saved = runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(saved.progress.attempts().len(), 2);
    assert_eq!(saved.progress.charged_tokens(), 0);
}
#[tokio::test]
async fn cancellation_removes_the_wake_and_never_resumes_it() {
    let f = Fixture::new(2).await;
    let run = f.park().await;
    let before = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(
        f.runtime.store.cancel(&run.id).await.unwrap().state,
        RunState::Canceled
    );
    assert_eq!(f.index_count(&run.id).await, 0);
    f.due(&run.id).await;
    assert_eq!(f.runtime.store.wake_workflow_timers().await.unwrap(), 0);
    assert!(f.runtime.store.claim_for_worker().await.unwrap().is_none());
    let after = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(
        canonical_hash(&before.progress),
        canonical_hash(&after.progress)
    );
}
#[tokio::test]
async fn revocation_during_wait_and_after_queueing_prevents_new_work() {
    for after_queue in [false, true] {
        let f = Fixture::new(2).await;
        let run = f.park().await;
        f.due(&run.id).await;
        if after_queue {
            assert_eq!(f.runtime.store.wake_workflow_timers().await.unwrap(), 1);
        }
        f.runtime
            .store
            .revoke_workflow(&f.project.id, &f.request.workflow_id, 1, "revoke")
            .await
            .unwrap();
        if after_queue {
            assert!(f.runtime.store.claim_for_worker().await.unwrap().is_none());
        } else {
            assert_eq!(f.runtime.store.wake_workflow_timers().await.unwrap(), 1);
        }
        let stopped = f.runtime.store.stored_run(&run.id).await.unwrap().record;
        assert_eq!(stopped.state, RunState::Failed);
        assert_eq!(stopped.error_code.as_deref(), Some("workflow_revoked"));
        assert!(stopped.model_attempts.is_empty());
        assert_eq!(f.index_count(&run.id).await, 0);
    }
}
#[tokio::test]
async fn known_bad_wait_input_is_rejected_before_a_run_is_admitted() {
    let f = Fixture::new(2).await;
    for value in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!(86401),
        json!("2"),
        Value::Null,
    ] {
        let mut input = f.request.clone();
        input.inputs = Some(Box::new(json!({"delay":value})));
        assert!(
            f.runtime
                .prepare_execution_plan(&RunRequest::Workflow(input))
                .await
                .is_err()
        );
    }
    let pool = f.pool().await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    pool.close().await;
}

#[tokio::test]
async fn a_known_native_prefix_rejects_bad_wait_data_before_creating_a_run() {
    let mut f = Fixture::new(0).await;
    let mut definition: WorkflowDefinition =
        serde_json::from_str(include_str!("../../../tests/fixtures/workflow-wait.json")).unwrap();
    definition.name = "Known native wait input".into();
    definition
        .controls
        .get_mut("wait")
        .unwrap()
        .dependencies
        .push("known_input".into());
    definition.controls.insert(
        "known_input".into(),
        WorkflowControlStep {
            name: "Prepare duration".into(),
            dependencies: vec![],
            operation: WorkflowControlOperation::Compose,
        },
    );
    let data = definition.dataflow.as_mut().unwrap();
    data.steps.insert(
        "known_input".into(),
        WorkflowStepData {
            context_policy: Default::default(),
            inputs: std::collections::BTreeMap::from([(
                "seconds".into(),
                WorkflowValueBinding {
                    value_type: WorkflowDataType::Integer,
                    value: WorkflowDataSource::Input {
                        pointer: "/delay".into(),
                    },
                },
            )]),
            output: WorkflowDataType::Object {
                fields: std::collections::BTreeMap::from([(
                    "seconds".into(),
                    WorkflowDataType::Integer,
                )]),
            },
        },
    );
    data.steps
        .get_mut("wait")
        .unwrap()
        .inputs
        .get_mut("seconds")
        .unwrap()
        .value = WorkflowDataSource::Step {
        step_id: "known_input".into(),
        pointer: "/seconds".into(),
    };
    let entry = f
        .runtime
        .store
        .create_workflow(&f.project.id, "known-native-prefix", &definition)
        .await
        .unwrap();
    f.request.workflow_id = entry.id;
    f.request.version_id = entry.versions[0].id.clone();
    assert!(
        f.runtime
            .prepare_execution_plan(&RunRequest::Workflow(f.request.clone()))
            .await
            .is_err()
    );
    let pool = f.pool().await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    pool.close().await;
    f.request.inputs = Some(Box::new(json!({"delay":2})));
    f.request.idempotency_key = "valid-known-native-prefix".into();
    let waiting = f.park().await;
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &waiting.id)
        .await
        .unwrap();
    assert_eq!(graph.progress.attempts().len(), 2);
    f.due(&waiting.id).await;
    assert_eq!(f.runtime.store.wake_workflow_timers().await.unwrap(), 1);
    let completed = f
        .runtime
        .run_native_workflow(&f.project.id, &waiting.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(completed.state, RunState::Completed);
    assert_eq!(completed.started_at, waiting.started_at);
    assert!(completed.model_attempts.is_empty());
}

async fn verify_interrupted_timer_resume(f: &Fixture) {
    let interrupted = f.interrupt_before_parking().await;
    assert_eq!(interrupted.state, RunState::Interrupted);
    let before = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &interrupted.id)
        .await
        .unwrap();
    let wake = before.progress.next_wake_at();
    assert!(wake.is_some());
    assert!(
        f.runtime
            .store
            .workflow_run_view(&f.project.id, &interrupted.id)
            .await
            .unwrap()
            .settled_resume_available
    );
    assert_eq!(
        f.runtime.store.resume(&interrupted.id).await.unwrap().state,
        RunState::Queued
    );
    assert!(f.runtime.store.resume(&interrupted.id).await.is_err());
    let parent = f
        .runtime
        .run_native_workflow(&f.project.id, &interrupted.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(parent.state, RunState::WaitingForTimer);
    assert_eq!(parent.started_at, interrupted.started_at);
    let saved = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &interrupted.id)
        .await
        .unwrap();
    assert_eq!(saved.progress.next_wake_at(), wake);
    assert_eq!(
        canonical_hash(saved.progress.attempts()),
        canonical_hash(before.progress.attempts())
    );
    f.due(&interrupted.id).await;
    assert_eq!(f.runtime.store.wake_workflow_timers().await.unwrap(), 1);
    let complete = f
        .runtime
        .run_native_workflow(&f.project.id, &interrupted.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(complete.state, RunState::Completed);
    assert_eq!(complete.started_at, interrupted.started_at);
    assert!(complete.model_attempts.is_empty());
}

#[tokio::test]
async fn interruption_after_timer_commit_before_parking_can_resume_without_restarting_it() {
    let f = Fixture::new(5).await;
    verify_interrupted_timer_resume(&f).await;
}
#[tokio::test]
async fn a_forged_early_projection_never_wakes_and_does_not_poison_the_queue() {
    let f = Fixture::new(5).await;
    let run = f.park().await;
    let pool = f.pool().await;
    sqlx::query("UPDATE workflow_timer_wakes SET wake_at=$1 WHERE run_id=$2")
        .bind(cowork_storage::now().div_euclid(1000) - 1)
        .bind(&run.id)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    assert_eq!(f.runtime.store.wake_workflow_timers().await.unwrap(), 0);
    assert!(f.runtime.store.claim_for_worker().await.unwrap().is_none());
    assert_eq!(
        f.runtime
            .store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .state,
        RunState::WaitingForTimer
    );
    let fast = f.fast_run().await;
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f.runtime.store, &fast.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(
        f.runtime.store.cancel(&run.id).await.unwrap().state,
        RunState::Canceled
    );
}

#[tokio::test]
async fn an_overdue_wait_expires_without_admitting_its_next_step() {
    let mut f = Fixture::new(2).await;
    f.request.budgets.max_seconds = 4;
    let run = f.park().await;
    let remaining = (run.started_at.unwrap() + 4100).saturating_sub(cowork_storage::now());
    if remaining > 0 {
        tokio::time::sleep(Duration::from_millis(remaining as u64)).await;
    }
    assert_eq!(f.runtime.store.wake_workflow_timers().await.unwrap(), 1);
    let expired = f.runtime.store.stored_run(&run.id).await.unwrap().record;
    assert_eq!(expired.state, RunState::Expired);
    assert_eq!(expired.error_code.as_deref(), Some("budget_exceeded"));
    assert_eq!(f.index_count(&run.id).await, 0);
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(graph.progress.attempts().len(), 1);
    assert!(matches!(
        graph.progress.nodes()["after"].state,
        WorkflowNodeState::Blocked
    ));
}

#[tokio::test]
async fn rollback_blocks_a_due_wait_but_still_allows_explicit_cancellation() {
    let f = Fixture::new(2).await;
    let run = f.park().await;
    f.due(&run.id).await;
    let db = f.pool().await;
    let (revision, water, payload): (i64, i64, String) = sqlx::query_as(
        "SELECT revision,highwater_ms,payload FROM workflow_execution_clocks WHERE run_id=$1",
    )
    .bind(&run.id)
    .fetch_one(&db)
    .await
    .unwrap();
    let mut value: Value = serde_json::from_str(&payload).unwrap();
    let raised = water.max(cowork_storage::now()) + 60000;
    value["highwater_ms"] = json!(raised);
    sqlx::query("UPDATE workflow_execution_clocks SET highwater_ms=$1,payload=$2 WHERE run_id=$3 AND revision=$4").bind(raised).bind(value.to_string()).bind(&run.id).bind(revision).execute(&db).await.unwrap();
    db.close().await;
    assert_eq!(f.runtime.store.wake_workflow_timers().await.unwrap(), 0);
    let blocked = f.runtime.store.stored_run(&run.id).await.unwrap().record;
    assert_eq!(blocked.state, RunState::WaitingForTimer);
    assert_eq!(
        blocked.error_code.as_deref(),
        Some("workflow_clock_rollback")
    );
    assert_eq!(
        f.runtime.store.cancel(&run.id).await.unwrap().state,
        RunState::Canceled
    );
    assert_eq!(f.index_count(&run.id).await, 0);
}

#[tokio::test]
#[ignore = "Requires an empty isolated cowork_timer_* PostgreSQL database on a private test host"]
async fn postgres_durable_timer_contract() {
    use std::str::FromStr;
    let url = std::env::var("COWORK_TEST_POSTGRES_TIMER").expect("dedicated timer database URL");
    let options = sqlx::postgres::PgConnectOptions::from_str(&url).unwrap();
    let host: std::net::IpAddr = options
        .get_host()
        .parse()
        .expect("literal private test address");
    assert!(
        host.is_loopback() || matches!(host, std::net::IpAddr::V4(address) if address.is_private())
    );
    assert!(
        options
            .get_database()
            .unwrap_or("")
            .starts_with("cowork_timer_")
    );
    let check = sqlx::PgPool::connect_with(options).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='public'"
        )
        .fetch_one(&check)
        .await
        .unwrap(),
        0,
        "The dedicated database must be empty before migration"
    );
    check.close().await;
    let f = Fixture::with_store(
        3,
        tempfile::tempdir().unwrap(),
        Store::connect(&url).await.unwrap(),
    )
    .await;
    let run = f.park().await;
    let wake = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap()
        .progress
        .next_wake_at();
    let activity = f.runtime.store.activity(false, 0, 20).await.unwrap();
    assert_eq!(activity.total, 1);
    assert_eq!(activity.attention, 0);
    assert_eq!(activity.items[0].run.id, run.id);
    f.runtime.store.close().await;
    let store = Store::connect(&url).await.unwrap();
    assert_eq!(
        store
            .stored_workflow(&f.project.id, &run.id)
            .await
            .unwrap()
            .progress
            .next_wake_at(),
        wake
    );
    let runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_file_broker(
            cowork_broker::FileBroker::open(&f.temp.path().join("broker"), vec![]).unwrap(),
        );
    let stop = CancellationToken::new();
    let worker = tokio::spawn(runtime.clone().serve(1, stop.clone()));
    let complete = state(&runtime.store, &run.id, RunState::Completed).await;
    assert_eq!(complete.started_at, run.started_at);
    stop.cancel();
    worker.await.unwrap();
    runtime.store.close().await;
    let f = Fixture::with_store(
        3,
        tempfile::tempdir().unwrap(),
        Store::connect(&url).await.unwrap(),
    )
    .await;
    let run = f.park().await;
    assert_eq!(
        f.runtime.store.cancel(&run.id).await.unwrap().state,
        RunState::Canceled
    );
    f.due(&run.id).await;
    assert_eq!(f.runtime.store.wake_workflow_timers().await.unwrap(), 0);
    f.runtime.store.close().await;
    let f = Fixture::with_store(
        2,
        tempfile::tempdir().unwrap(),
        Store::connect(&url).await.unwrap(),
    )
    .await;
    let run = f.park().await;
    f.due(&run.id).await;
    f.runtime
        .store
        .revoke_workflow(&f.project.id, &f.request.workflow_id, 1, "revoke-pg")
        .await
        .unwrap();
    assert_eq!(f.runtime.store.wake_workflow_timers().await.unwrap(), 1);
    assert_eq!(
        f.runtime
            .store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .state,
        RunState::Failed
    );
    f.runtime.store.close().await;
    let f = Fixture::with_store(
        5,
        tempfile::tempdir().unwrap(),
        Store::connect(&url).await.unwrap(),
    )
    .await;
    verify_interrupted_timer_resume(&f).await;
    f.runtime.store.close().await;
}
