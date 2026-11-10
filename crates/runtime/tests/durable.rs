use cowork_contracts::*;
use cowork_domain::Error;
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use std::time::Duration;

#[tokio::test]
async fn concurrent_duplicate_start_returns_one_run() {
    let (_dir, runtime, input) = setup().await;
    let mut starts = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let runtime = runtime.clone();
        let input = input.clone();
        starts.spawn(async move { runtime.create_run(&input).await });
    }
    let mut ids = std::collections::HashSet::new();
    while let Some(result) = starts.join_next().await {
        ids.insert(result.unwrap().unwrap().id);
    }
    assert_eq!(ids.len(), 1);
    assert_eq!(
        runtime.store.messages(&input.order_id).await.unwrap().len(),
        1
    );
    assert_eq!(runtime.store.runs(&input.order_id).await.unwrap().len(), 1);
}

use tokio_util::sync::CancellationToken;
async fn setup() -> (tempfile::TempDir, Runtime, CreateRun) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&temp.path().join("runtime.db"))
        .await
        .unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Runtime test".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "Generate a result".into(),
        })
        .await
        .unwrap();
    let profile = ProviderProfile {
        generation: Default::default(),
        id: "test".into(),
        name: "test".into(),
        kind: ProviderKind::Test,
        endpoint: "".into(),
        model: "test".into(),
        revision: 1,
        context_tokens: 32768,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        true,
    )
    .unwrap();
    let input = CreateRun {
        agent_config: None,
        model_binding: None,
        output: None,
        native_tools: vec![],
        environment: None,
        mcp_tools: vec![],
        budgets: cowork_contracts::Budgets::default(),
        folders: vec![],
        order_id: order.id,
        base_revision: 1,
        idempotency_key: "once".into(),
        prompt: "Create a result".into(),
        provider_id: "test".into(),
        provider_revision: 1,
        access_mode: AccessMode::ConfirmMutations,
    };
    (temp, runtime, input)
}
async fn state(runtime: &Runtime, id: &str, wanted: RunState) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if runtime.store.stored_run(id).await.unwrap().record.state == wanted {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("state deadline");
}
#[tokio::test]
async fn standard_approval_survives_worker_restart_and_is_exact() {
    let (_dir, runtime, input) = setup().await;
    let run = runtime.create_run(&input).await.unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(runtime.clone().serve(2, stop.clone()));
    state(&runtime, &run.id, RunState::WaitingApproval).await;
    assert!(
        runtime
            .store
            .artifacts(&input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    stop.cancel();
    worker.await.unwrap();
    let detail = runtime.store.detail(&run.id).await.unwrap();
    let approval = &detail.approvals[0];
    assert!(matches!(
        runtime
            .store
            .resolve_approval(
                &run.id,
                &approval.id,
                ResolveApproval {
                    fingerprint: "changed".into(),
                    allow: true
                }
            )
            .await,
        Err(Error::Conflict)
    ));
    runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint.clone(),
                allow: true,
            },
        )
        .await
        .unwrap();
    runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint.clone(),
                allow: true,
            },
        )
        .await
        .unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(runtime.clone().serve(2, stop.clone()));
    state(&runtime, &run.id, RunState::Completed).await;
    assert_eq!(
        runtime
            .store
            .artifacts(&input.order_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(runtime.create_run(&input).await.unwrap().id, run.id);
    assert_eq!(
        runtime.store.messages(&input.order_id).await.unwrap().len(),
        2
    );
    let events = runtime.store.events(&run.id, 0).await.unwrap();
    assert!(
        events
            .windows(2)
            .all(|e| e[1].sequence == e[0].sequence + 1)
    );
    stop.cancel();
    worker.await.unwrap();
}
#[tokio::test]
async fn denial_does_not_create_an_artifact() {
    let (_dir, runtime, input) = setup().await;
    let run = runtime.create_run(&input).await.unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(runtime.clone().serve(1, stop.clone()));
    state(&runtime, &run.id, RunState::WaitingApproval).await;
    let detail = runtime.store.detail(&run.id).await.unwrap();
    let a = &detail.approvals[0];
    runtime
        .store
        .resolve_approval(
            &run.id,
            &a.id,
            ResolveApproval {
                fingerprint: a.fingerprint.clone(),
                allow: false,
            },
        )
        .await
        .unwrap();
    state(&runtime, &run.id, RunState::Completed).await;
    assert!(
        runtime
            .store
            .artifacts(&input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    stop.cancel();
    worker.await.unwrap();
}
#[tokio::test]
async fn missing_profile_never_falls_back() {
    let (_dir, runtime, mut input) = setup().await;
    input.provider_id = "deleted".into();
    assert!(matches!(
        runtime.create_run(&input).await,
        Err(Error::Provider(_))
    ));
    assert!(
        runtime
            .store
            .runs(&input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn short_run(runtime: &Runtime, input: &CreateRun) -> RunRecord {
    let mut input = input.clone();
    input.budgets.max_seconds = 1;
    let input = &input;
    let flight = runtime.preflight(input).await.unwrap();
    let order = runtime.store.order(&input.order_id).await.unwrap();
    runtime
        .store
        .create_run(
            input,
            RunSpec {
                input_workspace: None,
                agent_config: None,
                model_binding: None,
                output: None,
                workflow: None,
                mcp_tools: vec![],
                environment: None,
                folders: flight.folders,
                order_id: order.id,
                project_id: order.project_id,
                prompt: input.prompt.clone(),
                provider: Some(flight.provider),
                file_action: None,
                access_mode: input.access_mode.clone(),
                executor: flight.executor,
                policy_revision: flight.policy_revision,
                allowed_tools: flight.allowed_tools,
                budgets: Budgets {
                    max_seconds: 1,
                    ..flight.budgets
                },
            },
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn time_budget_stops_a_model_request_before_its_reply() {
    let (_dir, runtime, input) = setup().await;
    runtime.providers.write().await[0].profile.model = "slow-test".into();
    let run = short_run(&runtime, &input).await;
    let stop = CancellationToken::new();
    let started = std::time::Instant::now();
    let worker = tokio::spawn(runtime.clone().serve(1, stop.clone()));
    state(&runtime, &run.id, RunState::Expired).await;
    assert!(started.elapsed() < Duration::from_secs(3)); // provider needs five seconds
    assert_eq!(
        runtime
            .store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .error_code
            .as_deref(),
        Some("budget_exceeded")
    );
    assert_eq!(
        runtime.store.messages(&input.order_id).await.unwrap().len(),
        1
    );
    assert!(
        runtime
            .store
            .artifacts(&input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    stop.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn unattended_approval_expires_and_cannot_authorize_a_late_action() {
    let (_dir, runtime, input) = setup().await;
    let run = short_run(&runtime, &input).await;
    let stop = CancellationToken::new();
    let worker = tokio::spawn(runtime.clone().serve(1, stop.clone()));
    state(&runtime, &run.id, RunState::WaitingApproval).await;
    let approval = runtime
        .store
        .detail(&run.id)
        .await
        .unwrap()
        .approvals
        .remove(0);
    state(&runtime, &run.id, RunState::Expired).await;
    assert!(matches!(
        runtime
            .store
            .resolve_approval(
                &run.id,
                &approval.id,
                ResolveApproval {
                    fingerprint: approval.fingerprint,
                    allow: true
                }
            )
            .await,
        Err(Error::BudgetExceeded)
    ));
    assert!(
        runtime
            .store
            .artifacts(&input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        runtime.store.detail(&run.id).await.unwrap().approvals[0]
            .decision
            .is_none()
    );
    stop.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn restart_expires_waiting_run_without_replaying_or_resetting_budget() {
    let (_dir, runtime, input) = setup().await;
    let run = short_run(&runtime, &input).await;
    let stop = CancellationToken::new();
    let worker = tokio::spawn(runtime.clone().serve(1, stop.clone()));
    state(&runtime, &run.id, RunState::WaitingApproval).await;
    stop.cancel();
    worker.await.unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let stop = CancellationToken::new();
    let restarted = tokio::spawn(runtime.clone().serve(1, stop.clone()));
    state(&runtime, &run.id, RunState::Expired).await;
    assert!(
        runtime
            .store
            .artifacts(&input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    stop.cancel();
    restarted.await.unwrap();
}
