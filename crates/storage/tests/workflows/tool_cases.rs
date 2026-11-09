use super::*;
use cowork_domain::{approval_fingerprint, canonical_hash};
use cowork_storage::{
    WorkflowModelAdmission, WorkflowModelResponse, WorkflowModelTurn, WorkflowToolExecution,
    WorkflowToolReceipt,
};
use serde_json::Value;
use std::time::Duration;

pub(super) async fn tool_fixture(store: &Store) -> Fixture {
    let mut f = setup(store).await;
    let rights = WorkflowRights {
        current_workspace: false,
        tools: BTreeSet::from([
            "create_artifact".into(),
            "list_artifacts".into(),
            "read_artifact".into(),
        ]),
        folders: BTreeSet::new(),
    };
    let profile = ProviderProfile {
        generation: Default::default(),
        id: id(),
        name: "Storage-only workflow tools fixture".into(),
        kind: ProviderKind::Test,
        endpoint: String::new(),
        model: "never-contacted-fixture".into(),
        revision: 1,
        context_tokens: 8192,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    store.insert_provider(&profile).await.unwrap();
    let limits = Budgets {
        max_steps: 32,
        max_seconds: 60,
        max_output_tokens: 256,
        max_total_tokens: 32768,
    };
    f.definition.rights = rights.clone();
    f.definition.budgets = limits.clone();
    for role in &mut f.definition.roles {
        role.rights = rights.clone();
        role.budgets = limits.clone();
        role.provider_id = profile.id.clone();
        role.provider_revision = profile.revision;
    }
    for step in &mut f.definition.steps {
        step.rights = rights.clone();
        step.budgets = limits.clone();
    }
    f.spec.allowed_tools = rights.tools.into_iter().collect();
    // The product scope has a frozen four-tool catalogue; agent rights remain three.
    f.spec.allowed_tools.push("request_input".into());
    f.spec.budgets = limits;
    let published = store
        .publish_workflow(
            &f.spec.project_id,
            &f.request.workflow_id,
            1,
            "publish-tool-fixture",
            &f.definition,
        )
        .await
        .unwrap();
    assert_eq!(published.revision, 2);
    f.request.version_id = published.version_id.unwrap();
    f
}

pub(super) fn catalog() -> Value {
    json!([
        {"type":"function","function":{"name":"create_artifact","parameters":{"type":"object"}}},
        {"type":"function","function":{"name":"list_artifacts","parameters":{"type":"object"}}},
        {"type":"function","function":{"name":"read_artifact","parameters":{"type":"object"}}}
    ])
}

fn create_call(raw_id: &str, name: &str, content: &str) -> ToolCall {
    ToolCall {
        id: raw_id.into(),
        name: "create_artifact".into(),
        arguments: json!({"name":name,"content":content}),
    }
}

pub(super) async fn save_proposal(
    store: &Store,
    f: &Fixture,
    run: &RunRecord,
    lease: &str,
    claim: &WorkflowClaim,
    calls: Vec<ToolCall>,
) -> WorkflowModelTurn {
    let (revision, context) = store
        .workflow_model_context(&f.spec.project_id, &run.id, claim, catalog())
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
                claim,
                &turn_id,
                &context,
            )
            .await
            .unwrap(),
        WorkflowModelAdmission::Dispatch(_)
    ));
    store
        .finish_workflow_model(
            &f.spec.project_id,
            &run.id,
            lease,
            claim,
            &turn_id,
            WorkflowModelResponse {
                provider_continuation: None,
                received_cost_metadata: None,
                content: "A saved tool proposal".into(),
                tools: calls,
                usage: Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(2),
                    ..Default::default()
                },
                rejection_code: None,
            },
        )
        .await
        .unwrap()
}

pub(super) async fn request_approval(
    store: &Store,
    f: &Fixture,
    run: &RunRecord,
    lease: &str,
    turn: &WorkflowModelTurn,
    raw_id: &str,
) -> Approval {
    match store
        .execute_workflow_artifact_tool(&f.spec.project_id, &run.id, lease, &turn.id, raw_id)
        .await
        .unwrap()
    {
        WorkflowToolExecution::AwaitingApproval(a) => a,
        other => panic!("expected exact approval, got {other:?}"),
    }
}

pub(super) async fn resolve(store: &Store, run: &RunRecord, approval: &Approval, allow: bool) {
    assert_eq!(
        store
            .resolve_approval(
                &run.id,
                &approval.id,
                ResolveApproval {
                    fingerprint: approval.fingerprint.clone(),
                    allow,
                },
            )
            .await
            .unwrap()
            .state,
        RunState::Running
    );
}

async fn execute(
    store: &Store,
    f: &Fixture,
    run: &RunRecord,
    lease: &str,
    turn: &WorkflowModelTurn,
    raw_id: &str,
) -> WorkflowToolReceipt {
    match store
        .execute_workflow_artifact_tool(&f.spec.project_id, &run.id, lease, &turn.id, raw_id)
        .await
        .unwrap()
    {
        WorkflowToolExecution::Completed(r) => r,
        other => panic!("expected committed receipt, got {other:?}"),
    }
}

async fn release_fixture(store: &Store, run: &RunRecord, lease: &str) {
    store.cancel(&run.id).await.unwrap();
    store
        .mutate(&run.id, Some(lease), |r| {
            r.transition(RunState::Canceled)?;
            r.lease = None;
            Ok(Change::event("tool_fixture_released", json!({})))
        })
        .await
        .unwrap();
}

pub(super) async fn contract(store: &Store) {
    let f = tool_fixture(store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let a = claim_node(store, &f, &run, &lease, "a").await;
    let b = claim_node(store, &f, &run, &lease, "b").await;
    let turn_a = save_proposal(
        store,
        &f,
        &run,
        &lease,
        &a,
        vec![create_call("shared-id", "approved.txt", "Approved result")],
    )
    .await;
    let turn_b = save_proposal(
        store,
        &f,
        &run,
        &lease,
        &b,
        vec![create_call("shared-id", "denied.txt", "Never create this")],
    )
    .await;
    assert!(
        store
            .artifacts(&run.spec.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        store
            .execute_workflow_artifact_tool(
                &f.spec.project_id,
                &run.id,
                "stale-owner",
                &turn_a.id,
                "shared-id",
            )
            .await,
        Err(Error::StaleLease)
    ));
    let foreign_project = store
        .create_project(CreateProject {
            name: "Foreign tools scope".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        store
            .execute_workflow_artifact_tool(
                &foreign_project.id,
                &run.id,
                &lease,
                &turn_a.id,
                "shared-id",
            )
            .await,
        Err(Error::NotFound)
    ));
    let graph_before_approval = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let approval_a = request_approval(store, &f, &run, &lease, &turn_a, "shared-id").await;
    let approval_b = request_approval(store, &f, &run, &lease, &turn_b, "shared-id").await;
    assert_ne!(approval_a.id, approval_b.id);
    assert_ne!(approval_a.tool.id, approval_b.tool.id);
    assert_ne!(approval_a.fingerprint, approval_b.fingerprint);
    assert_eq!(approval_a.tool.id.len(), 64);
    let mut scoped = run.clone();
    scoped.spec = store
        .workflow_node_spec(&f.spec.project_id, &run.id, "a")
        .await
        .unwrap();
    assert_eq!(
        approval_a.fingerprint,
        approval_fingerprint(&scoped, &approval_a.tool)
    );
    assert!(matches!(
        store
            .resolve_approval(
                &run.id,
                &approval_a.id,
                ResolveApproval {
                    fingerprint: approval_b.fingerprint.clone(),
                    allow: true
                },
            )
            .await,
        Err(Error::Conflict)
    ));
    let owner_before_resolve = store.stored_run(&run.id).await.unwrap();
    resolve(store, &run, &approval_a, true).await;
    resolve(store, &run, &approval_b, false).await;
    let owner_after_resolve = store.stored_run(&run.id).await.unwrap();
    assert_eq!(owner_after_resolve.lease.as_ref().unwrap().token, lease);
    assert_eq!(
        owner_after_resolve.lease.as_ref().unwrap().until,
        owner_before_resolve.lease.as_ref().unwrap().until
    );
    assert_eq!(
        owner_after_resolve.record.started_at,
        owner_before_resolve.record.started_at
    );
    assert_eq!(
        canonical_hash(
            &store
                .stored_workflow(&f.spec.project_id, &run.id)
                .await
                .unwrap()
        ),
        canonical_hash(&graph_before_approval),
        "approval decisions must not overwrite agent contexts or graph state",
    );
    assert!(matches!(
        store
            .resolve_approval(
                &run.id,
                &approval_a.id,
                ResolveApproval {
                    fingerprint: approval_a.fingerprint.clone(),
                    allow: false
                },
            )
            .await,
        Err(Error::Conflict)
    ));
    let before_effect = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let approved = execute(store, &f, &run, &lease, &turn_a, "shared-id").await;
    assert!(approved.succeeded);
    assert_eq!(approved.tool.id, "shared-id");
    assert_eq!(approved.id, approval_a.tool.id);
    let after_effect = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert_eq!(after_effect.format_version, 2);
    assert_eq!(
        after_effect.progress.revision(),
        before_effect.progress.revision() + 1
    );
    assert_eq!(
        after_effect.progress.attempts()[&approved.id]
            .response_sha256
            .as_deref(),
        Some(canonical_hash(&approved).as_str())
    );
    let denied = execute(store, &f, &run, &lease, &turn_b, "shared-id").await;
    assert!(!denied.succeeded);
    assert_eq!(denied.result, json!({"error":"policy_denied"}));
    let artifacts = store.artifacts(&run.spec.order_id).await.unwrap();
    assert_eq!(artifacts.len(), 1);
    let artifact = &artifacts[0];
    assert_eq!(artifact.run_id, run.id);
    assert_eq!(artifact.order_id, run.spec.order_id);
    assert_eq!(artifact.name, "approved.txt");
    assert_eq!(artifact.content, "Approved result");
    assert_eq!(approved.result["artifact_id"], artifact.id);
    assert_eq!(approved.result["sha256"], artifact.sha256);
    let sequence = store
        .stored_run(&run.id)
        .await
        .unwrap()
        .record
        .last_sequence;
    let replay = execute(store, &f, &run, "obsolete-owner", &turn_a, "shared-id").await;
    assert_eq!(replay, approved);
    assert_eq!(
        store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .last_sequence,
        sequence
    );
    assert_eq!(store.artifacts(&run.spec.order_id).await.unwrap().len(), 1);
    let graph = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert!(matches!(
        store
            .workflow_command(
                &f.spec.project_id,
                &run.id,
                &lease,
                graph.progress.revision(),
                WorkflowCommand::Settle {
                    claim: a.clone(),
                    id: approved.id.clone(),
                    outcome: WorkflowOutcome::Succeeded
                },
            )
            .await,
        Err(Error::Forbidden)
    ));
    store.heartbeat(&run.id, &lease).await.unwrap();

    let other_order = store
        .create_order(CreateOrder {
            project_id: f.spec.project_id.clone(),
            title: "Private other order".into(),
        })
        .await
        .unwrap();
    let mut other_request = f.request.clone();
    other_request.order_id = other_order.id.clone();
    other_request.base_revision = other_order.revision;
    other_request.idempotency_key = "other-order-tool".into();
    let mut other_spec = f.spec.clone();
    other_spec.order_id = other_order.id;
    other_spec.access_mode = AccessMode::Autonomous;
    let other_run = store
        .create_workflow_run(&other_request, other_spec)
        .await
        .unwrap();
    let other_parent = store
        .claim_workflow_run(&f.spec.project_id, &other_run.id)
        .await
        .unwrap();
    let other_lease = other_parent.lease.unwrap().token;
    let other_claim = claim_node(store, &f, &other_run, &other_lease, "a").await;
    let other_turn = save_proposal(
        store,
        &f,
        &other_run,
        &other_lease,
        &other_claim,
        vec![create_call(
            "shared-id",
            "foreign.txt",
            "Private foreign content",
        )],
    )
    .await;
    let other_result = execute(
        store,
        &f,
        &other_run,
        &other_lease,
        &other_turn,
        "shared-id",
    )
    .await;
    assert_ne!(other_result.id, approved.id);
    assert!(matches!(
        store
            .execute_workflow_artifact_tool(
                &f.spec.project_id,
                &other_run.id,
                &other_lease,
                &turn_a.id,
                "shared-id"
            )
            .await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store
            .resolve_approval(
                &other_run.id,
                &approval_a.id,
                ResolveApproval {
                    fingerprint: approval_a.fingerprint.clone(),
                    allow: true
                }
            )
            .await,
        Err(Error::NotFound)
    ));
    let foreign_id = other_result.result["artifact_id"].as_str().unwrap();
    let reads = save_proposal(
        store,
        &f,
        &run,
        &lease,
        &a,
        vec![
            ToolCall {
                id: "list-next".into(),
                name: "list_artifacts".into(),
                arguments: json!({}),
            },
            ToolCall {
                id: "read-next".into(),
                name: "read_artifact".into(),
                arguments: json!({"id":artifact.id}),
            },
            ToolCall {
                id: "read-foreign".into(),
                name: "read_artifact".into(),
                arguments: json!({"id":foreign_id}),
            },
        ],
    )
    .await;
    assert!(matches!(
        store
            .execute_workflow_artifact_tool(
                &f.spec.project_id,
                &run.id,
                &lease,
                &reads.id,
                "read-next"
            )
            .await,
        Err(Error::Conflict)
    ));
    let listed = execute(store, &f, &run, &lease, &reads, "list-next").await;
    assert!(listed.succeeded);
    assert_eq!(listed.result.as_array().unwrap().len(), 1);
    assert_eq!(listed.result[0]["id"], artifact.id);
    let read = execute(store, &f, &run, &lease, &reads, "read-next").await;
    assert!(read.succeeded);
    assert_eq!(read.result["content"], "Approved result");
    let blocked_read = execute(store, &f, &run, &lease, &reads, "read-foreign").await;
    assert!(!blocked_read.succeeded);
    assert_eq!(blocked_read.result, json!({"error":"not_found"}));
    assert!(
        !blocked_read
            .result
            .to_string()
            .contains("Private foreign content")
    );
    let (_, context) = store
        .workflow_model_context(&f.spec.project_id, &run.id, &a, catalog())
        .await
        .unwrap();
    assert!(
        context
            .messages
            .iter()
            .any(|m| m["role"] == "tool" && m["tool_call_id"] == "read-next")
    );
    assert!(
        context
            .messages
            .iter()
            .any(|m| m["role"] == "tool" && m["tool_call_id"] == "shared-id")
    );
    assert_eq!(store.artifacts(&run.spec.order_id).await.unwrap().len(), 1);
    let events = store.events(&run.id, 0).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "workflow_tool_completed")
            .count(),
        5
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "artifact_created")
            .count(),
        1
    );
    let export = store.export_order(&run.spec.order_id).await.unwrap();
    assert!(export.data.workflows.iter().any(|w| {
        w.agent_contexts
            .as_ref()
            .is_some_and(|a| a["a"]["turns"][0]["tool_results"][0]["id"] == approved.id)
    }));
    release_fixture(store, &other_run, &other_lease).await;
    release_fixture(store, &run, &lease).await;
}

pub(super) async fn postgres_approval_lease_fence(store: &Store, url: &str) {
    let f = tool_fixture(store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let claim = claim_node(store, &f, &run, &lease, "a").await;
    let turn = save_proposal(
        store,
        &f,
        &run,
        &lease,
        &claim,
        vec![create_call(
            "lease-call",
            "never-created.txt",
            "No effect after expiry",
        )],
    )
    .await;
    let approval = request_approval(store, &f, &run, &lease, &turn, "lease-call").await;
    let before = store.stored_run(&run.id).await.unwrap();
    let graph_before = store
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
    let approval_id = approval.id.clone();
    let fingerprint = approval.fingerprint.clone();
    let task = tokio::spawn(async move {
        owned
            .resolve_approval(
                &run_id,
                &approval_id,
                ResolveApproval {
                    fingerprint,
                    allow: true,
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'UPDATE workflows%' AND $1=ANY(pg_blocking_pids(pid))",
            ).bind(blocker).fetch_one(&pool).await.unwrap();
            if blocked > 0 { break; }
            assert!(!task.is_finished(), "approval must reach its own blocked workflow head");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(15_200)).await;
    assert!(!task.is_finished());
    lock.commit().await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(Error::StaleLease)));
    let after = store.stored_run(&run.id).await.unwrap();
    assert_eq!(after.record.revision, before.record.revision);
    assert_eq!(after.record.last_sequence, before.record.last_sequence);
    assert_eq!(
        after.lease.as_ref().unwrap().token,
        before.lease.as_ref().unwrap().token
    );
    assert_eq!(
        after.lease.as_ref().unwrap().until,
        before.lease.as_ref().unwrap().until
    );
    assert!(after.lease.as_ref().unwrap().until <= now());
    assert_eq!(
        after
            .checkpoint
            .approvals
            .iter()
            .find(|a| a.id == approval.id)
            .unwrap()
            .decision,
        None
    );
    assert_eq!(
        canonical_hash(
            &store
                .stored_workflow(&f.spec.project_id, &run.id)
                .await
                .unwrap()
        ),
        canonical_hash(&graph_before)
    );
    assert!(
        store
            .artifacts(&run.spec.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.recover_expired_leases().await.unwrap() > 0);
    assert_eq!(
        store.stored_run(&run.id).await.unwrap().record.state,
        RunState::Interrupted
    );
    let events = store.events(&run.id, 0).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "run_interrupted")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "approval_resolved")
            .count(),
        0
    );
    assert!(
        store
            .artifacts(&run.spec.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    store.cancel(&run.id).await.unwrap();
    pool.close().await;
}

#[tokio::test]
async fn rehashing_a_tool_result_cannot_replace_its_original_settlement_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tool-integrity.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let f = tool_fixture(&store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let claim = claim_node(&store, &f, &run, &lease, "a").await;
    let turn = save_proposal(
        &store,
        &f,
        &run,
        &lease,
        &claim,
        vec![create_call(
            "integrity-call",
            "original.txt",
            "Original committed content",
        )],
    )
    .await;
    let approval = request_approval(&store, &f, &run, &lease, &turn, "integrity-call").await;
    resolve(&store, &run, &approval, true).await;
    execute(&store, &f, &run, &lease, &turn, "integrity-call").await;
    let pool = raw(&path).await;
    let body: String = sqlx::query_scalar("SELECT payload FROM workflow_runs WHERE run_id=$1")
        .bind(&run.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut tampered: Value = serde_json::from_str(&body).unwrap();
    let receipt = &mut tampered["agents"]["a"]["turns"][0]["tool_results"][0];
    receipt["result"] = json!({"artifact_id":"replaced","sha256":"untrusted"});
    receipt["result_sha256"] = json!(canonical_hash(&receipt["result"]));
    sqlx::query("UPDATE workflow_runs SET payload=$1 WHERE run_id=$2")
        .bind(tampered.to_string())
        .bind(&run.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store.stored_workflow(&f.spec.project_id, &run.id).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        store.artifacts(&run.spec.order_id).await.unwrap()[0].content,
        "Original committed content"
    );
    pool.close().await;
    store.close().await;
}

#[tokio::test]
async fn artifact_receipt_and_event_rollback_together_before_an_explicit_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tool-atomic.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let f = tool_fixture(&store).await;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let claim = claim_node(&store, &f, &run, &lease, "a").await;
    let turn = save_proposal(
        &store,
        &f,
        &run,
        &lease,
        &claim,
        vec![create_call("atomic-call", "once.txt", "Exactly once")],
    )
    .await;
    let approval = request_approval(&store, &f, &run, &lease, &turn, "atomic-call").await;
    resolve(&store, &run, &approval, true).await;
    let before = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let parent_before = store.stored_run(&run.id).await.unwrap();
    let pool = raw(&path).await;
    sqlx::query("CREATE TRIGGER reject_tool_artifact AFTER INSERT ON artifacts BEGIN SELECT RAISE(ABORT,'controlled artifact failure'); END")
        .execute(&pool).await.unwrap();
    assert!(matches!(
        store
            .execute_workflow_artifact_tool(
                &f.spec.project_id,
                &run.id,
                &lease,
                &turn.id,
                "atomic-call"
            )
            .await,
        Err(Error::Storage(_))
    ));
    assert_eq!(
        canonical_hash(
            &store
                .stored_workflow(&f.spec.project_id, &run.id)
                .await
                .unwrap()
        ),
        canonical_hash(&before)
    );
    assert_eq!(
        store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .last_sequence,
        parent_before.record.last_sequence
    );
    assert!(
        store
            .artifacts(&run.spec.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("DROP TRIGGER reject_tool_artifact")
        .execute(&pool)
        .await
        .unwrap();
    let receipt = execute(&store, &f, &run, &lease, &turn, "atomic-call").await;
    assert!(receipt.succeeded);
    assert_eq!(store.artifacts(&run.spec.order_id).await.unwrap().len(), 1);
    let replay = execute(&store, &f, &run, "old-owner", &turn, "atomic-call").await;
    assert_eq!(receipt, replay);
    assert_eq!(
        store
            .events(&run.id, 0)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.kind == "workflow_tool_completed")
            .count(),
        1
    );
    release_fixture(&store, &run, &lease).await;
    pool.close().await;
    store.close().await;
}
