use super::*;
use cowork_domain::canonical_hash;
use cowork_workflow_contracts::{WorkflowNodeStatus, WorkflowRunKind, WorkflowRunRequest};
use std::collections::BTreeMap;
fn request(f: &Fixture) -> WorkflowRunRequest {
    WorkflowRunRequest {
        input_source: None,
        inputs: None,
        agent_config: None,
        native_tools: vec![],
        folders: vec![],
        mcp_tools: vec![],
        environment: None,
        kind: WorkflowRunKind::Workflow,
        order_id: f.spec.order_id.clone(),
        base_revision: f.request.base_revision,
        idempotency_key: "product-start".into(),
        prompt: f.spec.prompt.clone(),
        workflow_id: f.request.workflow_id.clone(),
        version_id: f.request.version_id.clone(),
        access_mode: f.spec.access_mode.clone(),
        budgets: f.spec.budgets.clone(),
        expected_binding: None,
    }
}
fn binding(input: &WorkflowRunRequest, plan: &WorkflowPlan) -> WorkflowBinding {
    WorkflowBinding {
        inputs: None,
        workflow_id: input.workflow_id.clone(),
        version_id: input.version_id.clone(),
        definition_sha256: plan.source_sha256.clone(),
        plan_sha256: plan.sha256.clone(),
    }
}
async fn product_run(
    store: &Store,
    f: &Fixture,
) -> (WorkflowRunRequest, WorkflowBinding, RunRecord) {
    let input = request(f);
    let plan = store
        .product_preflight_compilation(&input, &f.spec)
        .await
        .unwrap();
    assert_eq!(plan.scope.max_parallel, 4);
    let expected = binding(&input, &plan);
    let catalogues: BTreeMap<_, _> = plan
        .nodes
        .keys()
        .map(|id| (id.clone(), tool_cases::catalog()))
        .collect();
    let contexts = store
        .preview_product_workflow_contexts(&input, &f.spec, &plan, &catalogues)
        .await
        .unwrap();
    assert_eq!(contexts.len(), 2);
    assert!(
        contexts
            .iter()
            .all(|c| c.messages.iter().all(|m| m["content"].as_str().is_some()))
    );
    let run = store
        .create_product_workflow_run(&input, f.spec.clone(), &expected)
        .await
        .unwrap();
    (input, expected, run)
}
async fn awaiting(store: &Store, f: &Fixture, run: &RunRecord) -> (String, Approval) {
    let parent = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let claim = claim_node(store, f, run, &lease, "a").await;
    let turn = tool_cases::save_proposal(
        store,
        f,
        run,
        &lease,
        &claim,
        vec![ToolCall {
            id: "shared-raw-id".into(),
            name: "create_artifact".into(),
            arguments: json!({"name":"Output","content":"Fixture content"}),
        }],
    )
    .await;
    let approval =
        tool_cases::request_approval(store, f, run, &lease, &turn, "shared-raw-id").await;
    (lease, approval)
}
// Heap-own each independent phase instead of nesting the complete product and
// legacy SQLx state machines in the PostgreSQL test's default thread stack.
pub(super) async fn contract(store: &Store) {
    let f = Box::pin(tool_cases::tool_fixture(store)).await;
    let (input, expected, run) = Box::pin(product_run(store, &f)).await;
    Box::pin(identity_contract(store, &f, &input, &expected, &run)).await;
    Box::pin(attention_contract(store, &f, &run)).await;
    Box::pin(start_guards_contract(store)).await;
    Box::pin(deadline_attention_contract(store)).await;
    Box::pin(stop_attention_contract(store)).await;
    Box::pin(lease_attention_contract(store)).await;
}
async fn identity_contract(
    store: &Store,
    f: &Fixture,
    input: &WorkflowRunRequest,
    expected: &WorkflowBinding,
    run: &RunRecord,
) {
    let view = store
        .workflow_run_view(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert_eq!(view.version_revision, 2);
    assert!(view.approvals.is_empty());
    assert!(
        view.nodes
            .iter()
            .any(|n| n.state == WorkflowNodeStatus::Ready)
    );
    let serialized = serde_json::to_string(&view).unwrap();
    for forbidden in [
        "\"claim\":",
        "\"token\":",
        "\"agents\":",
        "\"turns\":",
        "\"messages\":",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "private checkpoint leaked: {forbidden}"
        );
    }
    assert_eq!(
        store
            .create_product_workflow_run(input, f.spec.clone(), expected)
            .await
            .unwrap()
            .id,
        run.id
    );
    let mut changed = input.clone();
    changed.prompt.push_str(" changed");
    assert!(matches!(
        store.existing_product_workflow_request(&changed).await,
        Err(Error::Conflict)
    ));
    let mut private = f.request.clone();
    private.idempotency_key = input.idempotency_key.clone();
    assert!(matches!(
        store.create_workflow_run(&private, f.spec.clone()).await,
        Err(Error::Conflict)
    ));
    let page = store
        .list_workflows(&f.spec.project_id, 0, 1)
        .await
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items.len(), 1);
    assert!(
        store
            .list_workflows(&f.spec.project_id, 0, 0)
            .await
            .is_err()
    );
    let foreign = store
        .create_project(CreateProject {
            name: "Foreign scope".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        store.workflow_run_view(&foreign.id, &run.id).await,
        Err(Error::NotFound)
    ));
    assert_eq!(
        store.list_workflows(&foreign.id, 0, 5).await.unwrap().total,
        0
    );
}
async fn attention_contract(store: &Store, f: &Fixture, run: &RunRecord) {
    let baseline = store.activity(true, 0, 100).await.unwrap().attention;
    let (lease, approval) = awaiting(store, f, run).await;
    let live = store
        .workflow_run_view(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert_eq!(live.run.state, RunState::Running);
    assert_eq!(live.approvals.len(), 1);
    assert_eq!(live.approvals[0].node_id, "a");
    assert_eq!(live.approvals[0].role_id.as_deref(), Some("author"));
    assert_eq!(live.approvals[0].approval.id, approval.id);
    assert_eq!(
        live.nodes.iter().find(|n| n.id == "a").unwrap().state,
        WorkflowNodeStatus::WaitingApproval
    );
    assert!(store.stored_run(&run.id).await.unwrap().lease.is_some());
    let f2 = tool_cases::tool_fixture(store).await;
    let (_, _, second) = product_run(store, &f2).await;
    let (_lease2, approval2) = awaiting(store, &f2, &second).await;
    let f3 = tool_cases::tool_fixture(store).await;
    let (_, _, plain) = product_run(store, &f3).await;
    let first = store.activity(true, 0, 1).await.unwrap();
    assert_eq!(first.attention, baseline + 2);
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.next_offset, Some(1));
    assert_ne!(first.items[0].run.id, plain.id);
    let next = store.activity(true, 1, 1).await.unwrap();
    assert_eq!(next.attention, baseline + 2);
    assert_eq!(next.items.len(), 1);
    assert_ne!(first.items[0].run.id, next.items[0].run.id);
    assert_ne!(next.items[0].run.id, plain.id);
    let all = store.activity(false, 0, 100).await.unwrap();
    assert!(
        all.items.iter().position(|i| i.run.id == run.id).unwrap()
            < all.items.iter().position(|i| i.run.id == plain.id).unwrap()
    );
    // Both snapshot readers and the authority owner may update the derived row.
    // PostgreSQL serialization retry must rebuild the entire count/page snapshot.
    let mut readers = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let reader = store.clone();
        let count = baseline + 2;
        readers.spawn(async move {
            for _ in 0..3 {
                let page = reader.activity(true, 0, 1).await.unwrap();
                assert_eq!(page.attention, count);
                assert_eq!(page.items.len(), 1);
                assert_eq!(page.next_offset, Some(1));
            }
        });
    }
    for _ in 0..3 {
        store.heartbeat(&run.id, &lease).await.unwrap();
    }
    while let Some(result) = readers.join_next().await {
        result.unwrap();
    }
    tool_cases::resolve(store, run, &approval, false).await;
    assert!(
        store
            .workflow_run_view(&f.spec.project_id, &run.id)
            .await
            .unwrap()
            .approvals
            .is_empty()
    );
    assert_eq!(
        store.activity(true, 0, 100).await.unwrap().attention,
        baseline + 1
    );
    store
        .revoke_workflow(
            &f2.spec.project_id,
            &f2.request.workflow_id,
            2,
            "product-revoke",
        )
        .await
        .unwrap();
    assert!(
        store
            .workflow_run_view(&f2.spec.project_id, &second.id)
            .await
            .unwrap()
            .approvals
            .is_empty()
    );
    assert_eq!(
        store.activity(true, 0, 100).await.unwrap().attention,
        baseline
    );
    assert!(
        store
            .resolve_approval(
                &second.id,
                &approval2.id,
                ResolveApproval {
                    fingerprint: approval2.fingerprint,
                    allow: true
                }
            )
            .await
            .is_err()
    );
    let mut invalid = f2.spec.clone();
    invalid.executor = "unavailable-derived-profile".into();
    assert_eq!(
        store
            .create_product_workflow_run(
                &request(&f2),
                invalid,
                &second.spec.workflow.clone().unwrap()
            )
            .await
            .unwrap()
            .id,
        second.id
    );
    store
        .mutate(&run.id, Some(&lease), |r| {
            r.lease.as_mut().unwrap().until = now() - 1;
            Ok(Change::event("fixture_lease_elapsed", json!({})))
        })
        .await
        .unwrap();
    assert!(
        store
            .workflow_run_view(&f.spec.project_id, &run.id)
            .await
            .unwrap()
            .approvals
            .is_empty()
    );
}
async fn start_guards_contract(store: &Store) {
    let untouched = tool_cases::tool_fixture(store).await;
    let input = request(&untouched);
    let plan = store
        .product_preflight_compilation(&input, &untouched.spec)
        .await
        .unwrap();
    let mut wrong = binding(&input, &plan);
    wrong.plan_sha256 = "b".repeat(64);
    let before = canonical_hash(&store.order(&input.order_id).await.unwrap());
    assert!(matches!(
        store
            .create_product_workflow_run(&input, untouched.spec.clone(), &wrong)
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        canonical_hash(&store.order(&input.order_id).await.unwrap()),
        before
    );
    assert!(store.messages(&input.order_id).await.unwrap().is_empty());
    let mut overbroad = untouched.spec.clone();
    overbroad.allowed_tools.push("shell".into());
    assert!(matches!(
        store
            .create_product_workflow_run(&input, overbroad, &binding(&input, &plan))
            .await,
        Err(Error::Forbidden)
    ));
    let mut definition = untouched.definition.clone();
    // Hierarchical workflows are supported; use an actually unknown capability
    // to exercise the product boundary instead of rejecting a shipped mode.
    definition
        .rights
        .tools
        .insert("unsupported_product_capability".into());
    let publication = store
        .publish_workflow(
            &untouched.spec.project_id,
            &untouched.request.workflow_id,
            2,
            "unsupported-product",
            &definition,
        )
        .await
        .unwrap();
    let mut unsupported = input.clone();
    unsupported.version_id = publication.version_id.unwrap();
    assert!(
        matches!(store.product_preflight_compilation(&unsupported,&untouched.spec).await,Err(Error::Validation(message)) if message=="workflow_capability_unavailable")
    );
    assert_eq!(
        canonical_hash(&store.order(&input.order_id).await.unwrap()),
        before
    );
}
async fn deadline_attention_contract(store: &Store) {
    let baseline = store.activity(true, 0, 100).await.unwrap().attention;
    let mut f = tool_cases::tool_fixture(store).await;
    f.definition.roles[0].budgets.max_seconds = 2;
    for step in &mut f.definition.steps {
        step.budgets.max_seconds = 2;
    }
    let publication = store
        .publish_workflow(
            &f.spec.project_id,
            &f.request.workflow_id,
            2,
            "short-approval-node",
            &f.definition,
        )
        .await
        .unwrap();
    f.request.version_id = publication.version_id.unwrap();
    let (_, _, run) = product_run(store, &f).await;
    let (_, approval) = awaiting(store, &f, &run).await;
    assert_eq!(
        store.activity(true, 0, 100).await.unwrap().attention,
        baseline + 1
    );
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    assert!(
        store
            .workflow_run_view(&f.spec.project_id, &run.id)
            .await
            .unwrap()
            .approvals
            .is_empty()
    );
    let attention = store.activity(true, 0, 100).await.unwrap();
    assert_eq!(attention.attention, baseline);
    assert!(attention.items.iter().all(|item| item.run.id != run.id));
    assert_eq!(
        store.stored_run(&run.id).await.unwrap().record.state,
        RunState::Running
    );
    assert!(
        store
            .resolve_approval(
                &run.id,
                &approval.id,
                ResolveApproval {
                    fingerprint: approval.fingerprint,
                    allow: true
                }
            )
            .await
            .is_err()
    );
}
async fn stop_attention_contract(store: &Store) {
    let baseline = store.activity(true, 0, 100).await.unwrap().attention;
    let mut f = tool_cases::tool_fixture(store).await;
    f.definition.failure = WorkflowFailure::Stop;
    let p = store
        .publish_workflow(
            &f.spec.project_id,
            &f.request.workflow_id,
            2,
            "stop-projection",
            &f.definition,
        )
        .await
        .unwrap();
    f.request.version_id = p.version_id.unwrap();
    let (_, _, run) = product_run(store, &f).await;
    let (lease, approval) = awaiting(store, &f, &run).await;
    assert_eq!(
        store.activity(true, 0, 100).await.unwrap().attention,
        baseline + 1
    );
    let b = claim_node(store, &f, &run, &lease, "b").await;
    let before = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    store
        .workflow_command(
            &f.spec.project_id,
            &run.id,
            &lease,
            before.progress.revision(),
            WorkflowCommand::Fail {
                claim: b,
                code: "known_failure".into(),
            },
        )
        .await
        .unwrap();
    let view = store
        .workflow_run_view(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert!(view.approvals.is_empty());
    assert!(
        view.nodes
            .iter()
            .all(|n| n.state != WorkflowNodeStatus::WaitingApproval)
    );
    let attention = store.activity(true, 0, 100).await.unwrap();
    assert_eq!(attention.attention, baseline);
    assert!(attention.items.iter().all(|item| item.run.id != run.id));
    assert_eq!(view.run.state, RunState::Running);
    assert!(
        store
            .resolve_approval(
                &run.id,
                &approval.id,
                ResolveApproval {
                    fingerprint: approval.fingerprint,
                    allow: true
                }
            )
            .await
            .is_err()
    );
}
async fn lease_attention_contract(store: &Store) {
    let baseline = store.activity(true, 0, 100).await.unwrap().attention;
    let f = tool_cases::tool_fixture(store).await;
    let (_, _, run) = product_run(store, &f).await;
    let (lease, approval) = awaiting(store, &f, &run).await;
    assert_eq!(
        store.activity(true, 0, 100).await.unwrap().attention,
        baseline + 1
    );
    store
        .mutate(&run.id, Some(&lease), |r| {
            r.lease.as_mut().unwrap().until = now() - 1;
            Ok(Change::event(
                "fixture_pending_approval_lease_elapsed",
                json!({}),
            ))
        })
        .await
        .unwrap();
    let view = store
        .workflow_run_view(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert!(view.approvals.is_empty());
    assert_eq!(view.run.state, RunState::Running);
    let attention = store.activity(true, 0, 100).await.unwrap();
    assert_eq!(attention.attention, baseline);
    assert!(attention.items.iter().all(|item| item.run.id != run.id));
    assert!(
        store
            .resolve_approval(
                &run.id,
                &approval.id,
                ResolveApproval {
                    fingerprint: approval.fingerprint,
                    allow: true
                }
            )
            .await
            .is_err()
    );
}
#[tokio::test]
async fn expired_workflow_approval_attention_is_removed_on_read_without_changing_parent_state() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    deadline_attention_contract(&store).await;
    store.close().await;
}
#[tokio::test]
async fn stopped_workflow_approval_is_not_projected_as_actionable() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    stop_attention_contract(&store).await;
    store.close().await;
}
#[tokio::test]
async fn migration_backfills_legacy_workflow_attention_and_confirmed_product_replay_survives_provider_delete()
 {
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("current-source.db");
    let path = temp.path().join("schema16-fixture.db");
    let store = Store::sqlite(&source_path).await.unwrap();
    let f = tool_cases::tool_fixture(&store).await;
    let (input, expected, run) = product_run(&store, &f).await;
    let (_, approval) = awaiting(&store, &f, &run).await;
    let original_parent = store.stored_run(&run.id).await.unwrap();
    let original_workflow = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let original_charges = store.detail(&run.id).await.unwrap().token_budget;
    assert_eq!(
        canonical_hash(&original_parent.checkpoint.approvals[0]),
        canonical_hash(&approval)
    );
    store.close().await;
    legacy_fixture::copy(&source_path, &path, 16).await;
    let store = Store::sqlite(&path).await.unwrap();
    let pool = raw(&path).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_execution_clocks")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0,
        "A legacy migration must not fabricate an execution clock"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='workflow_attention_actionable'")
            .fetch_one(&pool).await.unwrap(),
        1
    );
    pool.close().await;
    // The historical attention row/index is migrated exactly. A committed
    // product read suppresses execution actions without a verified clock.
    assert_eq!(store.activity(true, 0, 10).await.unwrap().attention, 1);
    let view = store
        .workflow_run_view(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert!(view.approvals.is_empty());
    assert!(matches!(
        store
            .resolve_approval(
                &run.id,
                &approval.id,
                ResolveApproval {
                    fingerprint: approval.fingerprint.clone(),
                    allow: true,
                }
            )
            .await,
        Err(Error::WorkflowClockUnverified)
    ));
    let restored_parent = store.stored_run(&run.id).await.unwrap();
    assert_eq!(
        canonical_hash(&restored_parent),
        canonical_hash(&original_parent)
    );
    assert_eq!(
        canonical_hash(&restored_parent.checkpoint.approvals[0]),
        canonical_hash(&approval)
    );
    assert_eq!(
        canonical_hash(
            &store
                .stored_workflow(&f.spec.project_id, &run.id)
                .await
                .unwrap()
        ),
        canonical_hash(&original_workflow)
    );
    assert_eq!(
        canonical_hash(&store.detail(&run.id).await.unwrap().token_budget),
        canonical_hash(&original_charges)
    );
    store
        .revoke_workflow(
            &f.spec.project_id,
            &f.request.workflow_id,
            2,
            "revoke-before-replay",
        )
        .await
        .unwrap();
    let pool = raw(&path).await;
    sqlx::query("DELETE FROM providers WHERE id=$1")
        .bind(&f.definition.roles[0].provider_id)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let existing = store
        .existing_product_workflow_request(&input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(existing.id, run.id);
    assert_eq!(
        canonical_hash(&existing),
        canonical_hash(&original_parent.record)
    );
    let replay = store
        .create_product_workflow_run(&input, f.spec.clone(), &expected)
        .await
        .unwrap();
    assert_eq!(replay.id, run.id);
    assert_eq!(
        canonical_hash(&replay),
        canonical_hash(&original_parent.record)
    );
    assert_eq!(
        canonical_hash(
            &store
                .stored_workflow(&f.spec.project_id, &run.id)
                .await
                .unwrap()
        ),
        canonical_hash(&original_workflow)
    );
    assert_eq!(
        canonical_hash(&store.detail(&run.id).await.unwrap().token_budget),
        canonical_hash(&original_charges)
    );
    let pool = raw(&path).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_execution_clocks")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    pool.close().await;
    store.close().await;
}

#[tokio::test]
async fn trusted_product_binding_rechecks_provider_body_even_without_public_expectation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profile-drift.db");
    let store = Store::sqlite(&path).await.unwrap();
    let f = tool_cases::tool_fixture(&store).await;
    let input = request(&f);
    assert!(input.expected_binding.is_none());
    let plan = store
        .product_preflight_compilation(&input, &f.spec)
        .await
        .unwrap();
    let expected = binding(&input, &plan);
    let order_before = canonical_hash(&store.order(&input.order_id).await.unwrap());
    let pool = raw(&path).await;
    let text: String = sqlx::query_scalar("SELECT payload FROM providers WHERE id=$1")
        .bind(&f.definition.roles[0].provider_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut profile: ProviderProfile = serde_json::from_str(&text).unwrap();
    profile.context_tokens = 16384;
    sqlx::query("UPDATE providers SET payload=$1 WHERE id=$2")
        .bind(serde_json::to_string(&profile).unwrap())
        .bind(&profile.id)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    assert!(matches!(
        store
            .create_product_workflow_run(&input, f.spec.clone(), &expected)
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        canonical_hash(&store.order(&input.order_id).await.unwrap()),
        order_before
    );
    assert!(store.runs(&input.order_id).await.unwrap().is_empty());
    assert!(store.messages(&input.order_id).await.unwrap().is_empty());
    store.close().await;
}
#[tokio::test]
async fn canceled_parent_does_not_project_a_historical_node_as_executing() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let f = tool_cases::tool_fixture(&store).await;
    let (_, _, run) = product_run(&store, &f).await;
    let (lease, _) = awaiting(&store, &f, &run).await;
    store.cancel(&run.id).await.unwrap();
    store
        .mutate(&run.id, Some(&lease), |r| {
            r.transition(RunState::Canceled)?;
            r.lease = None;
            Ok(Change::event("fixture_confirmed_cancel", json!({})))
        })
        .await
        .unwrap();
    let view = store
        .workflow_run_view(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert!(view.approvals.is_empty());
    assert_eq!(
        view.nodes.iter().find(|n| n.id == "a").unwrap().state,
        WorkflowNodeStatus::Interrupted
    );
    assert!(matches!(
        store
            .stored_workflow(&f.spec.project_id, &run.id)
            .await
            .unwrap()
            .progress
            .nodes()["a"]
            .state,
        WorkflowNodeState::Running { .. }
    ));
    store.close().await;
}
#[tokio::test]
#[ignore = "Owned hard-kill actor invoked by product_approval_attention_survives_hard_worker_end"]
async fn product_attention_process_fixture() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("COWORK_WORKFLOW_PRODUCT_ATTENTION_FIXTURE").unwrap(),
    );
    let store = Store::sqlite(&directory.join("attention.db"))
        .await
        .unwrap();
    let f = tool_cases::tool_fixture(&store).await;
    let (_, _, run) = product_run(&store, &f).await;
    let (_, approval) = awaiting(&store, &f, &run).await;
    assert_eq!(store.activity(true, 0, 10).await.unwrap().attention, 1);
    let bytes = serde_json::to_vec(
        &json!({"run_id":run.id,"project_id":f.spec.project_id,"approval_id":approval.id}),
    )
    .unwrap();
    let mut file = std::fs::File::create(directory.join("confirmed.json")).unwrap();
    use std::io::Write;
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
}
#[test]
fn product_approval_attention_survives_hard_worker_end() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let own = tempfile::tempdir().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "product_cases::product_attention_process_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("COWORK_WORKFLOW_PRODUCT_ATTENTION_FIXTURE", own.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = Child(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(20);
    let confirmed: serde_json::Value = loop {
        if let Ok(bytes) = std::fs::read(own.path().join("confirmed.json"))
            && let Ok(value) = serde_json::from_slice(&bytes)
        {
            break value;
        }
        assert!(child.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    };
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let path = own.path().join("attention.db");
        assert!(
            path.canonicalize()
                .unwrap()
                .starts_with(own.path().canonicalize().unwrap())
        );
        let store = Store::sqlite(&path).await.unwrap();
        let run = confirmed["run_id"].as_str().unwrap();
        let project = confirmed["project_id"].as_str().unwrap();
        let before = store.stored_run(run).await.unwrap();
        assert_eq!(before.record.state, RunState::Running);
        let lease = before.lease.as_ref().unwrap().token.clone();
        store
            .mutate(run, Some(&lease), |r| {
                r.lease.as_mut().unwrap().until = now() - 1;
                Ok(Change::event("fixture_elapsed_original_lease", json!({})))
            })
            .await
            .unwrap();
        store.recover_expired_leases().await.unwrap();
        let view = store.workflow_run_view(project, run).await.unwrap();
        assert_eq!(view.run.state, RunState::Interrupted);
        assert!(view.approvals.is_empty());
        assert!(!view.completion_resume_available);
        assert_eq!(
            view.nodes.iter().find(|n| n.id == "a").unwrap().state,
            WorkflowNodeStatus::Interrupted
        );
        let page = store.activity(true, 0, 10).await.unwrap();
        assert_eq!(page.attention, 1);
        assert_eq!(page.items[0].run.id, run);
        assert!(
            store
                .resolve_approval(
                    run,
                    confirmed["approval_id"].as_str().unwrap(),
                    ResolveApproval {
                        fingerprint: before.checkpoint.approvals[0].fingerprint.clone(),
                        allow: true
                    }
                )
                .await
                .is_err()
        );
        let pool = raw(&path).await;
        let projected: i64 =
            sqlx::query_scalar("SELECT actionable_count FROM workflow_attention WHERE run_id=$1")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(projected, 0);
        pool.close().await;
        assert!(
            store
                .artifacts(&view.run.spec.order_id)
                .await
                .unwrap()
                .is_empty()
        );
        store.close().await;
    });
}
