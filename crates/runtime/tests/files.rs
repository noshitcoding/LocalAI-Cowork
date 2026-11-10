use cowork_broker::{FileBroker, FolderConfig, PreparedWrite};
use cowork_contracts::*;
use cowork_domain::{Error, approval_fingerprint, hash};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::{Change, Store, now};
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

struct Fixture {
    dir: tempfile::TempDir,
    runtime: Runtime,
    input: CreateRun,
    tool: ToolCall,
}
async fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("work");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("note.txt"), "before").unwrap();
    let store = Store::sqlite(&dir.path().join("runtime.db")).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "File tests".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "File order".into(),
        })
        .await
        .unwrap();
    let broker = FileBroker::open(
        &dir.path().join("broker"),
        vec![FolderConfig {
            id: "root".into(),
            project_id: project.id.clone(),
            name: "Test folder".into(),
            path: folder,
            writable: true,
        }],
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
        order_id: order.id,
        base_revision: 1,
        idempotency_key: "file-start".into(),
        prompt: "Change the test document".into(),
        provider_id: "test".into(),
        provider_revision: 1,
        access_mode: AccessMode::ConfirmMutations,
        folders: broker.grants(&project.id).unwrap(),
    };
    let profile = ProviderProfile {
        generation: Default::default(),
        id: "test".into(),
        name: "Fixture".into(),
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
    .unwrap()
    .with_file_broker(broker);
    let tool = ToolCall {
        id: "write-1".into(),
        name: "write_text_file".into(),
        arguments: json!({"folder_id":"root","path":"note.txt","content":"after","expected_sha256":hash(b"before")}),
    };
    Fixture {
        dir,
        runtime,
        input,
        tool,
    }
}
async fn seed(f: &Fixture, input: &CreateRun, tool: &ToolCall) -> RunRecord {
    let run = f.runtime.create_run(input).await.unwrap();
    seed_tool(f, &run.id, tool).await.unwrap();
    run
}
async fn seed_tool(f: &Fixture, run_id: &str, tool: &ToolCall) -> cowork_domain::Result<()> {
    f.runtime.store.mutate(run_id,None,|r| {
        // Fixture proposals must be complete before a worker can read them.
        // A separate model response cannot replace an active worker checkpoint.
        if !matches!(r.record.state, RunState::Queued | RunState::WaitingForExecutor)
            || r.lease.is_some()
            || r.record.started_at.is_some()
            || !r.record.model_attempts.is_empty()
            || !r.checkpoint.pending_tools.is_empty()
            || r.checkpoint.in_flight.is_some()
            || r.checkpoint.completion_pending
        {
            return Err(Error::Conflict);
        }
        r.checkpoint.pending_tools=vec![tool.clone()];
        r.checkpoint.messages.push(json!({"role":"assistant","content":"Fixture tool request","tool_calls":[{"id":tool.id,"type":"function","function":{"name":tool.name,"arguments":tool.arguments.to_string()}}]}));
        Ok(Change::event("fixture_model_reply",json!({})))
    }).await.map(|_| ())
}

#[tokio::test]
async fn fixture_model_proposal_cannot_replace_an_active_worker_checkpoint() {
    let f = setup().await;
    let run = f.runtime.create_run(&f.input).await.unwrap();
    let claimed = f.runtime.store.claim_for_worker().await.unwrap().unwrap();
    assert_eq!(claimed.record.id, run.id);
    assert_eq!(claimed.record.state, RunState::Running);
    assert!(claimed.lease.is_some());
    let events = f.runtime.store.events(&run.id, 0).await.unwrap();
    assert!(matches!(
        seed_tool(&f, &run.id, &f.tool).await,
        Err(Error::Conflict)
    ));
    let after = f.runtime.store.stored_run(&run.id).await.unwrap();
    assert_eq!(after.record.revision, claimed.record.revision);
    assert_eq!(
        serde_json::to_value(&after.checkpoint).unwrap(),
        serde_json::to_value(&claimed.checkpoint).unwrap()
    );
    assert_eq!(
        serde_json::to_value(f.runtime.store.events(&run.id, 0).await.unwrap()).unwrap(),
        serde_json::to_value(events).unwrap()
    );
    assert_eq!(
        std::fs::read(f.dir.path().join("work/note.txt")).unwrap(),
        b"before"
    );
}

#[tokio::test]
async fn revoked_folder_is_denied_after_approval_without_blocking_the_orders_own_snapshot() {
    let f = setup().await;
    let run = seed(&f, &f.input, &f.tool).await;
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f, &run.id, RunState::WaitingApproval).await;
    let grant = &f.input.folders[0];
    f.runtime
        .files
        .as_ref()
        .unwrap()
        .revoke_native_folder(&grant.project_id, &grant.id, &grant.revision)
        .unwrap();
    approval(&f, &run.id, true).await;
    state(&f, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(
        std::fs::read(f.dir.path().join("work/note.txt")).unwrap(),
        b"before"
    );
    let detail = f.runtime.store.detail(&run.id).await.unwrap();
    assert!(detail.file_changes.is_empty());
    assert!(detail.workspace_snapshot.is_some());
    assert!(
        f.runtime
            .store
            .events(&run.id, 0)
            .await
            .unwrap()
            .iter()
            .any(|event| event.kind == "tool_completed"
                && event.payload["result"]["error"] == "forbidden")
    );
    let mut stale = f.input.clone();
    stale.idempotency_key = "new-start-with-revoked-grant".into();
    stale.base_revision = f
        .runtime
        .store
        .order(&stale.order_id)
        .await
        .unwrap()
        .revision;
    assert!(matches!(
        f.runtime.create_run(&stale).await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn revoked_folder_still_allows_exact_human_reconciliation_without_reauthorizing_tools() {
    let f = setup().await;
    let (run, _, guard) = uncertain(&f, true).await;
    drop(guard);
    let grant = &f.input.folders[0];
    f.runtime
        .files
        .as_ref()
        .unwrap()
        .revoke_native_folder(&grant.project_id, &grant.id, &grant.revision)
        .unwrap();
    let detail = f.runtime.store.detail(&run.id).await.unwrap();
    f.runtime
        .reconcile_file(
            &run.id,
            ReconcileFile {
                fingerprint: detail.uncertain_file_fingerprint.unwrap(),
                decision: ReconcileFileDecision::AcceptCompleted,
            },
        )
        .await
        .unwrap();
    let after = f.runtime.store.detail(&run.id).await.unwrap();
    assert_eq!(after.run.state, RunState::Interrupted);
    assert_eq!(after.file_changes.len(), 1);
    assert!(
        f.runtime
            .files
            .as_ref()
            .unwrap()
            .prepare_write(&after.run, &f.tool)
            .is_err()
    );
    f.runtime.store.cancel(&run.id).await.unwrap();
    assert_eq!(
        std::fs::read(f.dir.path().join("work/note.txt")).unwrap(),
        b"after"
    );
}
async fn state(f: &Fixture, id: &str, wanted: RunState) {
    if tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if f.runtime.store.stored_run(id).await.unwrap().record.state == wanted {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_err()
    {
        let last = f.runtime.store.stored_run(id).await.unwrap();
        let events = f.runtime.store.events(id, 0).await.unwrap();
        panic!(
            "Expected {wanted:?} for {id}; last={:?}, pending_tools={:?}, in_flight={:?}, completion_pending={}, events={events:?}",
            last.record,
            last.checkpoint.pending_tools,
            last.checkpoint.in_flight,
            last.checkpoint.completion_pending
        );
    }
}
async fn approval(f: &Fixture, id: &str, allow: bool) {
    state(f, id, RunState::WaitingApproval).await;
    let a = f
        .runtime
        .store
        .detail(id)
        .await
        .unwrap()
        .approvals
        .into_iter()
        .find(|a| a.decision.is_none())
        .unwrap();
    assert!(matches!(
        f.runtime
            .store
            .resolve_approval(
                id,
                &a.id,
                ResolveApproval {
                    fingerprint: "different arguments".into(),
                    allow
                }
            )
            .await,
        Err(Error::Conflict)
    ));
    f.runtime
        .store
        .resolve_approval(
            id,
            &a.id,
            ResolveApproval {
                fingerprint: a.fingerprint,
                allow,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn file_policy_modes_and_durable_versions_use_the_common_path() {
    for (mode, allow, changed) in [
        (AccessMode::ReadOnly, false, false),
        (AccessMode::ConfirmMutations, false, false),
        (AccessMode::ConfirmMutations, true, true),
        (AccessMode::ConfirmAll, true, true),
        (AccessMode::Autonomous, true, true),
    ] {
        let mut f = setup().await;
        f.input.access_mode = mode.clone();
        let run = seed(&f, &f.input, &f.tool).await;
        let stop = CancellationToken::new();
        let worker = tokio::spawn(f.runtime.clone().serve(2, stop.clone()));
        if matches!(mode, AccessMode::ConfirmMutations | AccessMode::ConfirmAll) {
            state(&f, &run.id, RunState::WaitingApproval).await;
            assert_eq!(
                std::fs::read_to_string(f.dir.path().join("work/note.txt")).unwrap(),
                "before"
            );
            approval(&f, &run.id, allow).await;
        }
        state(&f, &run.id, RunState::Completed).await;
        stop.cancel();
        worker.await.unwrap();
        let detail = f.runtime.store.detail(&run.id).await.unwrap();
        assert_eq!(detail.file_changes.len(), usize::from(changed));
        assert!(detail.uncertain_file_change.is_none());
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("work/note.txt")).unwrap(),
            if changed { "after" } else { "before" }
        );
        if changed {
            let old = f
                .runtime
                .previous_file_version(&run.id, &f.tool.id)
                .await
                .unwrap();
            assert_eq!(old.content, "before");
            assert_eq!(old.sha256, hash(b"before"));
            let reopened = Store::sqlite(&f.dir.path().join("runtime.db"))
                .await
                .unwrap();
            assert_eq!(
                reopened.detail(&run.id).await.unwrap().file_changes.len(),
                1
            );
            assert_eq!(
                reopened
                    .export_order(&f.input.order_id)
                    .await
                    .unwrap()
                    .data
                    .runs[0]
                    .file_changes
                    .len(),
                1
            );
        }
    }
}

// Simulate the precise crash boundary: journal committed, filesystem effect optional,
// no completion transaction. The real broker performs the effect in the successful case.
async fn uncertain(f: &Fixture, applied: bool) -> (RunRecord, String, PreparedWrite) {
    let mut input = f.input.clone();
    input.access_mode = AccessMode::Autonomous;
    let run = seed(f, &input, &f.tool).await;
    let claimed = f.runtime.store.claim().await.unwrap().unwrap();
    let token = claimed.lease.unwrap().token;
    let mut prepared = f
        .runtime
        .files
        .as_ref()
        .unwrap()
        .prepare_write(&claimed.record, &f.tool)
        .unwrap();
    let fingerprint = approval_fingerprint(&claimed.record, &f.tool);
    f.runtime
        .store
        .mutate(&run.id, Some(&token), |r| {
            r.checkpoint.in_flight = Some(f.tool.clone());
            r.checkpoint.pending_file_change = Some(prepared.change.clone());
            Ok(Change::event(
                "file_write_started",
                json!({"fingerprint":fingerprint}),
            ))
        })
        .await
        .unwrap();
    if applied {
        prepared.apply().unwrap();
    }
    f.runtime
        .store
        .mutate(&run.id, Some(&token), |r| {
            r.lease.as_mut().unwrap().until = now() - 1;
            Ok(Change::default())
        })
        .await
        .unwrap();
    assert_eq!(f.runtime.store.recover_expired_leases().await.unwrap(), 1);
    (run, token, prepared)
}

#[tokio::test]
async fn uncertain_write_is_fenced_reconciled_and_never_automatically_repeated() {
    let f = setup().await;
    let (run, token, guard) = uncertain(&f, true).await;
    let detail = f.runtime.store.detail(&run.id).await.unwrap();
    assert_eq!(detail.run.error_code.as_deref(), Some("uncertain_effect"));
    assert!(detail.file_changes.is_empty());
    assert!(detail.uncertain_file_change.is_some());
    assert!(matches!(
        f.runtime.store.resume(&run.id).await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        f.runtime.store.cancel(&run.id).await,
        Err(Error::UncertainEffect)
    ));
    assert!(matches!(
        f.runtime.store.heartbeat(&run.id, &token).await,
        Err(Error::StaleLease)
    ));
    let decision = ReconcileFile {
        fingerprint: detail.uncertain_file_fingerprint.unwrap(),
        decision: ReconcileFileDecision::AcceptCompleted,
    };
    assert!(matches!(
        f.runtime.reconcile_file(&run.id, decision.clone()).await,
        Err(Error::WorkspaceBusy)
    )); // old writer still alive
    drop(guard);
    let mut wrong = decision.clone();
    wrong.fingerprint = "changed".into();
    assert!(matches!(
        f.runtime.reconcile_file(&run.id, wrong).await,
        Err(Error::Conflict)
    ));
    let mut wrong = decision.clone();
    wrong.decision = ReconcileFileDecision::ConfirmNotApplied;
    assert!(matches!(
        f.runtime.reconcile_file(&run.id, wrong).await,
        Err(Error::FileConflict)
    ));
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(2, stop.clone()));
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        f.runtime.store.detail(&run.id).await.unwrap().run.state,
        RunState::Interrupted
    );
    f.runtime
        .reconcile_file(&run.id, decision.clone())
        .await
        .unwrap();
    assert!(matches!(
        f.runtime.reconcile_file(&run.id, decision).await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        f.runtime.store.detail(&run.id).await.unwrap().run.state,
        RunState::Interrupted
    );
    f.runtime.store.resume(&run.id).await.unwrap();
    state(&f, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    let detail = f.runtime.store.detail(&run.id).await.unwrap();
    assert_eq!(detail.file_changes.len(), 1);
    assert_eq!(
        f.runtime
            .store
            .events(&run.id, 0)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.kind == "file_write_started")
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("work/note.txt")).unwrap(),
        "after"
    );
}

#[tokio::test]
async fn unchanged_file_requires_explicit_reconciliation_and_resume() {
    let f = setup().await;
    let (run, _, guard) = uncertain(&f, false).await;
    drop(guard);
    let detail = f.runtime.store.detail(&run.id).await.unwrap();
    let mut decision = ReconcileFile {
        fingerprint: detail.uncertain_file_fingerprint.unwrap(),
        decision: ReconcileFileDecision::AcceptCompleted,
    };
    assert!(matches!(
        f.runtime.reconcile_file(&run.id, decision.clone()).await,
        Err(Error::FileConflict)
    ));
    decision.decision = ReconcileFileDecision::ConfirmNotApplied;
    f.runtime.reconcile_file(&run.id, decision).await.unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("work/note.txt")).unwrap(),
        "before"
    );
    f.runtime.store.resume(&run.id).await.unwrap();
    state(&f, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(
        f.runtime
            .store
            .detail(&run.id)
            .await
            .unwrap()
            .file_changes
            .len(),
        1
    );
}

#[tokio::test]
async fn writer_ownership_survives_approval_and_releases_after_cancel() {
    let f = setup().await;
    let first = seed(&f, &f.input, &f.tool).await;
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(3, stop.clone()));
    state(&f, &first.id, RunState::WaitingApproval).await;
    let mut second = f.input.clone();
    second.idempotency_key = "second".into();
    second.base_revision = f
        .runtime
        .store
        .order(&second.order_id)
        .await
        .unwrap()
        .revision;
    let second = seed(&f, &second, &f.tool).await;
    state(&f, &second.id, RunState::WaitingForExecutor).await;
    f.runtime.store.cancel(&first.id).await.unwrap();
    state(&f, &second.id, RunState::WaitingApproval).await;
    approval(&f, &second.id, true).await;
    state(&f, &second.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    assert!(
        f.runtime
            .store
            .detail(&first.id)
            .await
            .unwrap()
            .file_changes
            .is_empty()
    );
    assert_eq!(
        f.runtime
            .store
            .detail(&second.id)
            .await
            .unwrap()
            .file_changes
            .len(),
        1
    );
}

#[tokio::test]
async fn managed_files_persist_and_branch_at_an_older_result() {
    let mut f = setup().await;
    f.input.folders.clear();
    f.input.access_mode = AccessMode::Autonomous;
    let workspace = f.runtime.workspace(&f.input.order_id).await.unwrap();
    let mut tool = f.tool.clone();
    tool.id = "managed-first".into();
    tool.arguments = json!({"folder_id":workspace.folder.id,"path":"result.txt","content":"first","expected_sha256":null});
    let first = seed(&f, &f.input, &tool).await;
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(2, stop.clone()));
    state(&f, &first.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    let boundary = f
        .runtime
        .store
        .messages(&f.input.order_id)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.run_id == first.id && m.role == "assistant")
        .unwrap();
    f.input.base_revision = f
        .runtime
        .store
        .order(&f.input.order_id)
        .await
        .unwrap()
        .revision;
    f.input.idempotency_key = "second-managed".into();
    tool.id = "managed-second".into();
    tool.arguments["content"] = json!("second");
    tool.arguments["expected_sha256"] = json!(hash(b"first"));
    let second = seed(&f, &f.input, &tool).await;
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(2, stop.clone()));
    state(&f, &second.id, RunState::Completed).await;
    assert_eq!(
        f.runtime
            .workspace_text(&f.input.order_id, "result.txt".into())
            .await
            .unwrap()
            .content,
        "second"
    );
    let branch = f
        .runtime
        .store
        .branch_order(
            &f.input.order_id,
            BranchOrder {
                title: "Older files".into(),
                through_message_id: boundary.id.clone(),
                idempotency_key: "branch-files".into(),
            },
        )
        .await
        .unwrap();
    let child = f.runtime.workspace(&branch.id).await.unwrap();
    let first_snapshot = f
        .runtime
        .store
        .detail(&first.id)
        .await
        .unwrap()
        .workspace_snapshot
        .unwrap();
    assert_eq!(child.base_snapshot_sha256, Some(first_snapshot.sha256));
    assert_eq!(
        f.runtime
            .workspace_text(&branch.id, "result.txt".into())
            .await
            .unwrap()
            .content,
        "first"
    );
    let grandchild = f
        .runtime
        .store
        .branch_order(
            &branch.id,
            BranchOrder {
                title: "Inherited boundary".into(),
                through_message_id: boundary.id,
                idempotency_key: "grandchild".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.runtime
            .workspace_text(&grandchild.id, "result.txt".into())
            .await
            .unwrap()
            .content,
        "first"
    );
    let exported = f.runtime.store.export_order(&branch.id).await.unwrap();
    assert_eq!(exported.format_version, 13);
    assert_eq!(
        exported
            .data
            .history
            .inherited
            .unwrap()
            .workspace_snapshots
            .len(),
        1
    );
    let mut forged = f.input.clone();
    forged.folders = vec![child.folder];
    forged.base_revision = f
        .runtime
        .store
        .order(&forged.order_id)
        .await
        .unwrap()
        .revision;
    assert!(matches!(
        f.runtime.preflight(&forged).await,
        Err(Error::Forbidden)
    ));
    stop.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn snapshot_checkpoint_survives_restart_without_a_second_model_call() {
    let mut f = setup().await;
    f.input.folders.clear();
    let run = f.runtime.create_run(&f.input).await.unwrap();
    let claimed = f.runtime.store.claim().await.unwrap().unwrap();
    let token = claimed.lease.unwrap().token;
    f.runtime
        .store
        .mutate(&run.id, Some(&token), |r| {
            r.checkpoint.completion_pending = true;
            r.checkpoint.last_model_content = "Already received final response".into();
            r.record.steps = r.record.spec.budgets.max_steps;
            r.transition(RunState::WaitingForSnapshot)?;
            r.lease.as_mut().unwrap().until = now() - 1;
            Ok(Change::event("fixture_snapshot_checkpoint", json!({})))
        })
        .await
        .unwrap();
    f.runtime.store.close().await;
    let store = Store::sqlite(&f.dir.path().join("runtime.db"))
        .await
        .unwrap();
    let broker = FileBroker::open(&f.dir.path().join("broker"), vec![]).unwrap();
    // No providers configured: any accidental call to the model would fail this test.
    f.runtime = Runtime::new(store, vec![], true)
        .unwrap()
        .with_file_broker(broker);
    assert_eq!(f.runtime.store.recover_expired_leases().await.unwrap(), 1);
    f.runtime.store.resume(&run.id).await.unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    let detail = f.runtime.store.detail(&run.id).await.unwrap();
    assert!(detail.workspace_snapshot.is_some());
    assert_eq!(detail.run.steps, detail.run.spec.budgets.max_steps);
    let messages = f.runtime.store.messages(&f.input.order_id).await.unwrap();
    assert_eq!(
        messages.last().unwrap().content,
        "Already received final response"
    );
    assert_eq!(messages.iter().filter(|m| m.role == "assistant").count(), 1);
    assert!(
        !f.runtime
            .store
            .events(&run.id, 0)
            .await
            .unwrap()
            .iter()
            .any(|e| e.kind == "model_started")
    );
}

#[tokio::test]
async fn rename_uses_all_four_policy_modes_and_persists_its_receipt() {
    for (mode, allow, changed) in [
        (AccessMode::ReadOnly, false, false),
        (AccessMode::ConfirmMutations, false, false),
        (AccessMode::ConfirmMutations, true, true),
        (AccessMode::ConfirmAll, true, true),
        (AccessMode::Autonomous, true, true),
    ] {
        let mut f = setup().await;
        f.input.access_mode = mode.clone();
        let tool = ToolCall {
            id: "rename-fixture".into(),
            name: "rename_file".into(),
            arguments: json!({"folder_id":"root","path":"note.txt","destination":"renamed.txt","expected_sha256":hash(b"before")}),
        };
        let run = seed(&f, &f.input, &tool).await;
        let stop = CancellationToken::new();
        let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
        if matches!(mode, AccessMode::ConfirmMutations | AccessMode::ConfirmAll) {
            state(&f, &run.id, RunState::WaitingApproval).await;
            assert!(f.dir.path().join("work/note.txt").is_file());
            approval(&f, &run.id, allow).await;
        }
        state(&f, &run.id, RunState::Completed).await;
        stop.cancel();
        worker.await.unwrap();
        let detail = f.runtime.store.detail(&run.id).await.unwrap();
        assert_eq!(detail.file_operations.len(), usize::from(changed));
        assert!(detail.uncertain_file_operation.is_none());
        assert_eq!(f.dir.path().join("work/renamed.txt").is_file(), changed);
        assert_eq!(f.dir.path().join("work/note.txt").is_file(), !changed);
        if changed {
            assert_eq!(
                detail.file_operations[0].before.as_ref().unwrap().sha256,
                hash(b"before")
            );
        }
    }
}

#[tokio::test]
async fn every_file_operation_requires_exact_reconciliation_after_restart() {
    for kind in ["create_directory", "rename_file", "remove_file"] {
        for applied in [false, true] {
            let mut f = setup().await;
            f.input.access_mode = AccessMode::Autonomous;
            let arguments = match kind {
                "create_directory" => json!({"folder_id":"root","path":"new-folder"}),
                "rename_file" => {
                    json!({"folder_id":"root","path":"note.txt","destination":"renamed.txt","expected_sha256":hash(b"before")})
                }
                _ => {
                    json!({"folder_id":"root","path":"note.txt","expected_sha256":hash(b"before")})
                }
            };
            let tool = ToolCall {
                id: "interrupted-operation".into(),
                name: kind.into(),
                arguments,
            };
            let run = seed(&f, &f.input, &tool).await;
            let claimed = f.runtime.store.claim().await.unwrap().unwrap();
            let token = claimed.lease.unwrap().token;
            let prepared = f
                .runtime
                .files
                .as_ref()
                .unwrap()
                .prepare_operation(&claimed.record, &tool)
                .unwrap();
            f.runtime
                .store
                .mutate(&run.id, Some(&token), |r| {
                    r.checkpoint.in_flight = Some(tool.clone());
                    r.checkpoint.pending_file_operation = Some(prepared.operation.clone());
                    Ok(Change::event("file_operation_started", json!({})))
                })
                .await
                .unwrap();
            if applied {
                prepared.apply().unwrap();
            }
            f.runtime
                .store
                .mutate(&run.id, Some(&token), |r| {
                    r.lease.as_mut().unwrap().until = now() - 1;
                    Ok(Change::default())
                })
                .await
                .unwrap();
            assert_eq!(f.runtime.store.recover_expired_leases().await.unwrap(), 1);
            assert!(f.runtime.store.resume(&run.id).await.is_err());
            assert!(f.runtime.store.cancel(&run.id).await.is_err());
            let fingerprint = approval_fingerprint(&claimed.record, &tool);
            let decision = ReconcileFile {
                fingerprint: fingerprint.clone(),
                decision: if applied {
                    ReconcileFileDecision::AcceptCompleted
                } else {
                    ReconcileFileDecision::ConfirmNotApplied
                },
            };
            assert!(matches!(
                f.runtime.reconcile_file(&run.id, decision.clone()).await,
                Err(Error::WorkspaceBusy)
            ));
            drop(prepared);
            // A restarted broker has no managed root registered until reconciliation restores it.
            f.runtime.files = Some(std::sync::Arc::new(
                FileBroker::open(
                    &f.dir.path().join("broker"),
                    vec![FolderConfig {
                        id: "root".into(),
                        project_id: claimed.record.spec.project_id.clone(),
                        name: "Test folder".into(),
                        path: f.dir.path().join("work"),
                        writable: true,
                    }],
                )
                .unwrap(),
            ));
            let opposite = ReconcileFile {
                fingerprint,
                decision: if applied {
                    ReconcileFileDecision::ConfirmNotApplied
                } else {
                    ReconcileFileDecision::AcceptCompleted
                },
            };
            assert!(matches!(
                f.runtime.reconcile_file(&run.id, opposite).await,
                Err(Error::FileConflict)
            ));
            assert_eq!(
                f.runtime
                    .reconcile_file(&run.id, decision)
                    .await
                    .unwrap()
                    .state,
                RunState::Interrupted
            );
            assert_eq!(
                f.runtime
                    .store
                    .detail(&run.id)
                    .await
                    .unwrap()
                    .file_operations
                    .len(),
                usize::from(applied)
            );
            f.runtime.store.resume(&run.id).await.unwrap();
            let stop = CancellationToken::new();
            let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
            state(&f, &run.id, RunState::Completed).await;
            stop.cancel();
            worker.await.unwrap();
            let detail = f.runtime.store.detail(&run.id).await.unwrap();
            assert_eq!(detail.file_operations.len(), 1);
            assert!(detail.uncertain_file_operation.is_none());
            let events = f.runtime.store.events(&run.id, 0).await.unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e.kind == "file_operation_started")
                    .count(),
                if applied { 1 } else { 2 }
            );
        }
    }
}

#[tokio::test]
async fn native_restore_needs_no_provider_preserves_draft_and_obeys_policy_and_scope() {
    let mut f = setup().await;
    f.input.access_mode = AccessMode::Autonomous;
    let source = seed(&f, &f.input, &f.tool).await;
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f, &source.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    f.runtime.providers.write().await.clear();
    let order = f.runtime.store.order(&f.input.order_id).await.unwrap();
    let draft = f
        .runtime
        .store
        .save_draft(
            &order.id,
            SaveDraft {
                base_revision: order.revision,
                draft: "An unrelated important draft".into(),
            },
        )
        .await
        .unwrap();
    let input = CreateRestoreRun {
        order_id: order.id.clone(),
        base_revision: draft.revision,
        idempotency_key: "native-restore".into(),
        prompt: "Restore the previous file".into(),
        access_mode: AccessMode::ConfirmMutations,
        restore: RestoreFile {
            expected_source_sha256: None,
            folder_id: "root".into(),
            path: "restored.txt".into(),
            source: FileRestoreSource {
                run_id: source.id,
                tool_call_id: f.tool.id.clone(),
                version: RetainedVersion::TextBefore,
            },
        },
    };
    let mut readonly = input.clone();
    readonly.access_mode = AccessMode::ReadOnly;
    assert!(matches!(
        f.runtime.create_restore_run(&readonly).await,
        Err(Error::Forbidden)
    ));
    let foreign = f
        .runtime
        .store
        .create_order(CreateOrder {
            project_id: order.project_id.clone(),
            title: "Different order".into(),
        })
        .await
        .unwrap();
    let mut forged = input.clone();
    forged.order_id = foreign.id;
    forged.base_revision = 1;
    assert!(matches!(
        f.runtime.create_restore_run(&forged).await,
        Err(Error::Forbidden)
    ));
    let run = f.runtime.create_restore_run(&input).await.unwrap();
    assert!(run.spec.provider.is_none());
    assert!(run.spec.file_action.is_some());
    assert_eq!(
        f.runtime.create_restore_run(&input).await.unwrap().id,
        run.id
    );
    let mut changed = input.clone();
    changed.restore.path = "changed.txt".into();
    assert!(matches!(
        f.runtime.create_restore_run(&changed).await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        f.runtime.store.order(&order.id).await.unwrap().draft,
        draft.draft
    );
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f, &run.id, RunState::WaitingApproval).await;
    assert!(!f.dir.path().join("work/restored.txt").exists());
    approval(&f, &run.id, true).await;
    state(&f, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("work/restored.txt")).unwrap(),
        "before"
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("work/note.txt")).unwrap(),
        "after"
    );
    let detail = f.runtime.store.detail(&run.id).await.unwrap();
    assert_eq!(detail.file_operations.len(), 1);
    assert_eq!(
        detail.file_operations[0].restored_from.as_ref(),
        Some(&input.restore.source)
    );
    assert!(detail.workspace_snapshot.is_some());
    assert!(
        !f.runtime
            .store
            .events(&run.id, 0)
            .await
            .unwrap()
            .iter()
            .any(|e| e.kind.starts_with("model_"))
    );
    assert_eq!(
        f.runtime.create_restore_run(&input).await.unwrap().id,
        run.id
    );
    for (path, allow, expected) in [
        ("restored.txt", true, "file_conflict"),
        ("denied.txt", false, "policy_denied"),
    ] {
        let mut next = input.clone();
        next.idempotency_key = path.into();
        next.base_revision = f.runtime.store.order(&order.id).await.unwrap().revision;
        next.restore.path = path.into();
        let run = f.runtime.create_restore_run(&next).await.unwrap();
        let stop = CancellationToken::new();
        let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
        approval(&f, &run.id, allow).await;
        state(&f, &run.id, RunState::Failed).await;
        stop.cancel();
        worker.await.unwrap();
        let detail = f.runtime.store.detail(&run.id).await.unwrap();
        assert_eq!(detail.run.error_code.as_deref(), Some(expected));
        assert!(detail.file_operations.is_empty());
    }
    assert!(!f.dir.path().join("work/denied.txt").exists());
}

#[tokio::test]
async fn native_restore_reconciles_applied_and_unapplied_work_without_a_model() {
    for applied in [false, true] {
        let mut f = setup().await;
        f.input.access_mode = AccessMode::Autonomous;
        let source = seed(&f, &f.input, &f.tool).await;
        let stop = CancellationToken::new();
        let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
        state(&f, &source.id, RunState::Completed).await;
        stop.cancel();
        worker.await.unwrap();
        f.runtime.providers.write().await.clear();
        let input = CreateRestoreRun {
            order_id: f.input.order_id.clone(),
            base_revision: f
                .runtime
                .store
                .order(&f.input.order_id)
                .await
                .unwrap()
                .revision,
            idempotency_key: "restart-restoration".into(),
            prompt: "Restore once".into(),
            access_mode: AccessMode::Autonomous,
            restore: RestoreFile {
                expected_source_sha256: None,
                folder_id: "root".into(),
                path: "restored.txt".into(),
                source: FileRestoreSource {
                    run_id: source.id,
                    tool_call_id: f.tool.id.clone(),
                    version: RetainedVersion::TextBefore,
                },
            },
        };
        let run = f.runtime.create_restore_run(&input).await.unwrap();
        let claim = f.runtime.store.claim().await.unwrap().unwrap();
        let token = claim.lease.unwrap().token;
        let tool = run.spec.file_action.clone().unwrap();
        let prepared = f
            .runtime
            .files
            .as_ref()
            .unwrap()
            .prepare_restore(
                &run,
                &tool,
                &cowork_broker::RetainedContent::File {
                    sha256: hash(b"before"),
                },
            )
            .unwrap();
        f.runtime
            .store
            .mutate(&run.id, Some(&token), |r| {
                r.checkpoint.in_flight = Some(tool.clone());
                r.checkpoint.pending_file_operation = Some(prepared.operation.clone());
                Ok(Change::event("file_operation_started", json!({})))
            })
            .await
            .unwrap();
        if applied {
            prepared.apply().unwrap();
        }
        drop(prepared);
        f.runtime
            .store
            .mutate(&run.id, Some(&token), |r| {
                r.lease.as_mut().unwrap().until = now() - 1;
                Ok(Change::default())
            })
            .await
            .unwrap();
        assert_eq!(f.runtime.store.recover_expired_leases().await.unwrap(), 1);
        assert!(f.runtime.store.resume(&run.id).await.is_err());
        let decision = ReconcileFile {
            fingerprint: approval_fingerprint(&run, &tool),
            decision: if applied {
                ReconcileFileDecision::AcceptCompleted
            } else {
                ReconcileFileDecision::ConfirmNotApplied
            },
        };
        f.runtime.reconcile_file(&run.id, decision).await.unwrap();
        f.runtime.store.resume(&run.id).await.unwrap();
        let stop = CancellationToken::new();
        let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
        state(&f, &run.id, RunState::Completed).await;
        stop.cancel();
        worker.await.unwrap();
        assert_eq!(
            f.runtime
                .store
                .file_operations(&run.id)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            f.runtime
                .store
                .events(&run.id, 0)
                .await
                .unwrap()
                .iter()
                .filter(|e| e.kind == "file_operation_started")
                .count(),
            if applied { 1 } else { 2 }
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("work/restored.txt")).unwrap(),
            "before"
        );
    }
}
