use cowork_contracts::*;
use cowork_domain::*;
use cowork_storage::{Change, Store, id, now};
use serde_json::json;

async fn setup(store: &Store) -> (CreateRun, RunSpec) {
    let project = store
        .create_project(CreateProject {
            name: "Contract fixture".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Durability test".into(),
        })
        .await
        .unwrap();
    let profile = ProviderProfile {
        generation: Default::default(),
        id: "fixture".into(),
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
    let input = CreateRun {
        agent_config: None,
        model_binding: None,
        output: None,
        native_tools: vec![],
        environment: None,
        mcp_tools: vec![],
        budgets: cowork_contracts::Budgets::default(),
        folders: vec![],
        order_id: order.id.clone(),
        base_revision: order.revision,
        idempotency_key: id(),
        prompt: "Create a result".into(),
        provider_id: profile.id.clone(),
        provider_revision: 1,
        access_mode: AccessMode::ConfirmMutations,
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
        prompt: input.prompt.clone(),
        provider: Some(profile),
        file_action: None,
        access_mode: AccessMode::ConfirmMutations,
        executor: "local-v2".into(),
        policy_revision: 1,
        allowed_tools: vec!["create_artifact".into()],
        budgets: Budgets::default(),
    };
    (input, spec)
}
async fn common_contract(store: &Store) {
    let (input, spec) = setup(store).await;
    let run = store.create_run(&input, spec.clone()).await.unwrap();
    assert_eq!(
        store.create_run(&input, spec.clone()).await.unwrap().id,
        run.id
    );
    let mut changed = input.clone();
    changed.prompt = "Different request".into();
    assert!(matches!(
        store.create_run(&changed, spec).await,
        Err(Error::Conflict)
    ));
    assert_eq!(store.messages(&input.order_id).await.unwrap().len(), 1);
    let claim = store.claim().await.unwrap().unwrap();
    assert_eq!(claim.record.id, run.id);
    assert!(store.claim().await.unwrap().is_none());
    let token = &claim.lease.as_ref().unwrap().token;
    assert!(matches!(
        store
            .mutate(&run.id, Some("stale-worker"), |_| Ok(Change::default()))
            .await,
        Err(Error::StaleLease)
    ));
    store
        .mutate(&run.id, Some(token), |r| {
            r.transition(RunState::Completed)?;
            let artifact = Artifact {
                id: id(),
                run_id: r.record.id.clone(),
                order_id: r.record.spec.order_id.clone(),
                name: "answer.txt".into(),
                content: "Durable result".into(),
                size: 14,
                sha256: hash(b"Durable result"),
                media_type: "text/plain".into(),
                created_at: now(),
            };
            let mut change = Change::event("run_completed", json!({"state":"completed"}));
            change.artifacts.push(artifact);
            r.lease = None;
            Ok(change)
        })
        .await
        .unwrap();
    let events = store.events(&run.id, 0).await.unwrap();
    assert_eq!(
        events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(store.events(&run.id, 2).await.unwrap().len(), 1);
    assert_eq!(store.detail(&run.id).await.unwrap().artifacts.len(), 1);
    assert_eq!(
        store.stored_run(&run.id).await.unwrap().record.state,
        RunState::Completed
    );
    assert!(matches!(store.resume(&run.id).await, Err(Error::Conflict)));
}
#[tokio::test]
async fn sqlite_contract_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let store = Store::sqlite(&path).await.unwrap();
    common_contract(&store).await;
    let before = store.projects().await.unwrap();
    store.close().await;
    let reopened = Store::sqlite(&path).await.unwrap();
    assert_eq!(reopened.projects().await.unwrap()[0].id, before[0].id);
    let orders = reopened.orders(&before[0].id).await.unwrap();
    assert_eq!(
        reopened.artifacts(&orders[0].id).await.unwrap()[0].content,
        "Durable result"
    );
    reopened.close().await;
}
#[tokio::test]
#[ignore = "Requires a dedicated PostgreSQL database in COWORK_TEST_POSTGRES"]
async fn postgres_same_storage_contract() {
    let url =
        std::env::var("COWORK_TEST_POSTGRES").expect("dedicated PostgreSQL database required");
    let store = Store::connect(&url).await.unwrap();
    common_contract(&store).await;
    model_accounting_contract(&store).await;
    history_contract(&store).await;
    waiting_budget_contract(&store).await;
    file_ownership_contract(&store).await;
    workspace_completion_contract(&store).await;
    native_action_storage_contract(&store).await;
    upload_storage_contract(&store).await;
    store.close().await;
}

async fn model_accounting_contract(store: &Store) {
    let (mut input, mut spec) = setup(store).await;
    input.budgets.max_total_tokens = 65536;
    spec.budgets = input.budgets.clone();
    let run = store.create_run(&input, spec).await.unwrap();
    let claimed = store.claim().await.unwrap().unwrap();
    assert_eq!(claimed.record.id, run.id);
    let lease = claimed.lease.unwrap().token;
    let digest = hash(b"frozen logical request");
    let reserve = |r: &mut StoredRun| {
        let attempt = reserve_model(&mut r.record, &digest, "attempt-a", now())?;
        Ok(Change::event("model_started", json!({"attempt":attempt})))
    };
    let (first, second) = tokio::join!(
        store.mutate(&run.id, Some(&lease), reserve),
        store.mutate(&run.id, Some(&lease), reserve)
    );
    assert_eq!(
        [first.is_ok(), second.is_ok()]
            .iter()
            .filter(|ok| **ok)
            .count(),
        1
    );
    assert_eq!(
        store
            .detail(&run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        32768
    );
    assert!(matches!(
        store
            .mutate(&run.id, Some("old-worker"), |r| {
                settle_model(
                    &mut r.record,
                    "attempt-a",
                    &Usage {
                        input_tokens: Some(12),
                        output_tokens: Some(3),
                        ..Default::default()
                    },
                    now(),
                )?;
                Ok(Change::default())
            })
            .await,
        Err(Error::StaleLease)
    ));
    store
        .mutate(&run.id, Some(&lease), |r| {
            settle_model(
                &mut r.record,
                "attempt-a",
                &Usage {
                    input_tokens: Some(12),
                    output_tokens: Some(3),
                    ..Default::default()
                },
                now(),
            )?;
            Ok(Change::event(
                "model_completed",
                json!({"attempt_id":"attempt-a"}),
            ))
        })
        .await
        .unwrap();
    assert_eq!(
        store
            .detail(&run.id)
            .await
            .unwrap()
            .token_budget
            .charged_tokens,
        15
    );
    assert!(matches!(
        store
            .mutate(&run.id, Some(&lease), |r| {
                settle_model(&mut r.record, "attempt-a", &Usage::default(), now())?;
                Ok(Change::default())
            })
            .await,
        Err(Error::Conflict)
    ));
    store
        .mutate(&run.id, Some(&lease), |r| {
            reserve_model(&mut r.record, &digest, "attempt-b", now())?;
            Ok(Change::event(
                "model_started",
                json!({"attempt_id":"attempt-b"}),
            ))
        })
        .await
        .unwrap();
    store
        .mutate(&run.id, Some(&lease), |r| {
            settle_model(
                &mut r.record,
                "attempt-b",
                &Usage {
                    input_tokens: Some(5),
                    ..Default::default()
                },
                now(),
            )?;
            Ok(Change::event(
                "model_completed",
                json!({"attempt_id":"attempt-b"}),
            ))
        })
        .await
        .unwrap();
    let detail = store.detail(&run.id).await.unwrap();
    assert_eq!(detail.token_budget.charged_tokens, 32783);
    assert_eq!(detail.token_budget.unconfirmed_tokens, 32768);
    assert_eq!(detail.run.usage.input_tokens, Some(17));
    assert!(detail.run.usage.incomplete);
    assert!(matches!(
        store
            .mutate(&run.id, Some(&lease), |r| {
                reserve_model(&mut r.record, &digest, "attempt-c", now())?;
                Ok(Change::default())
            })
            .await,
        Err(Error::BudgetExceeded)
    ));
    store
        .mutate(&run.id, Some(&lease), |r| {
            r.transition(RunState::Completed)?;
            r.lease = None;
            Ok(Change::event("run_completed", json!({})))
        })
        .await
        .unwrap();
    let exported = store.export_order(&input.order_id).await.unwrap();
    assert_eq!(exported.data.runs[0].run.model_attempts.len(), 2);
    assert_eq!(exported.data.runs[0].token_budget.charged_tokens, 32783);
    assert_eq!(exported.sha256, canonical_hash(&exported.data));
}

#[tokio::test]
async fn model_reservation_settlement_and_export_are_atomic_and_lease_fenced() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    model_accounting_contract(&store).await;
}

#[tokio::test]
async fn failed_measurement_commit_retains_the_reservation_across_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("budgets.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let (input, spec) = setup(&store).await;
    let run = store.create_run(&input, spec).await.unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    let lease = claim.lease.unwrap().token;
    store
        .mutate(&run.id, Some(&lease), |r| {
            reserve_model(&mut r.record, &hash(b"request"), "attempt", now())?;
            Ok(Change::event("model_started", json!({})))
        })
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.display()))
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_measurement BEFORE INSERT ON events WHEN NEW.payload LIKE '%model_completed%' BEGIN SELECT RAISE(ABORT,'fixture failure'); END").execute(&pool).await.unwrap();
    assert!(
        store
            .mutate(&run.id, Some(&lease), |r| {
                settle_model(
                    &mut r.record,
                    "attempt",
                    &Usage {
                        input_tokens: Some(1),
                        output_tokens: Some(1),
                        ..Default::default()
                    },
                    now(),
                )?;
                Ok(Change::event("model_completed", json!({})))
            })
            .await
            .is_err()
    );
    pool.close().await;
    store.close().await;
    let reopened = Store::sqlite(&path).await.unwrap();
    let detail = reopened.detail(&run.id).await.unwrap();
    assert_eq!(detail.token_budget.charged_tokens, 32768);
    assert_eq!(
        detail.run.model_attempts[0].state,
        ModelAttemptState::InFlight
    );
    assert_eq!(detail.run.usage.input_tokens, None);
    reopened
        .mutate(&run.id, Some(&lease), |r| {
            r.transition(RunState::Interrupted)?;
            r.lease = None;
            Ok(Change::event("fixture_interrupted", json!({})))
        })
        .await
        .unwrap();
    let detail = reopened.detail(&run.id).await.unwrap();
    assert_eq!(
        detail.run.model_attempts[0].state,
        ModelAttemptState::Interrupted
    );
    assert_eq!(detail.token_budget.unconfirmed_tokens, 32768);
    assert!(detail.run.usage.incomplete);
}

async fn upload_storage_contract(store: &Store) -> String {
    let (input, mut spec) = setup(store).await;
    let reserved = ReserveUpload {
        idempotency_key: "upload".into(),
        file: DownloadMetadata {
            file_name: "Pr\u{00fc}fung \u{1f4c4}.bin".into(),
            size: 4,
            sha256: hash(&[0, 255, 128, 42]),
        },
    };
    let (a, b) = tokio::join!(
        store.reserve_upload(&input.order_id, &reserved),
        store.reserve_upload(&input.order_id, &reserved)
    );
    let upload = a.unwrap();
    assert_eq!(b.unwrap().id, upload.id);
    assert_eq!(upload.state, UploadState::Reserved);
    let mut changed = reserved.clone();
    changed.file.sha256 = hash(b"nope");
    assert!(matches!(
        store.reserve_upload(&input.order_id, &changed).await,
        Err(Error::Conflict)
    ));
    let (foreign, _) = setup(store).await;
    assert!(matches!(
        store.upload(&foreign.order_id, &upload.id).await,
        Err(Error::NotFound)
    ));
    let draft = store
        .save_draft(
            &input.order_id,
            SaveDraft {
                base_revision: 1,
                draft: "Important independent draft".into(),
            },
        )
        .await
        .unwrap();
    let import = ImportFile {
        folder_id: "managed".into(),
        path: "result.bin".into(),
        upload_id: upload.id.clone(),
        file: upload.file.clone(),
        expected_sha256: None,
    };
    let request = CreateImportRun {
        order_id: input.order_id.clone(),
        base_revision: draft.revision,
        idempotency_key: "import".into(),
        prompt: "Import binary".into(),
        access_mode: AccessMode::ConfirmMutations,
        import: import.clone(),
    };
    spec.prompt = request.prompt.clone();
    spec.provider = None;
    spec.allowed_tools = vec!["import_file".into()];
    spec.file_action = Some(ToolCall {
        id: "import".into(),
        name: "import_file".into(),
        arguments: json!(import),
    });
    assert!(matches!(
        store.create_import_run(&request, spec.clone()).await,
        Err(Error::Conflict)
    ));
    let (a, b) = tokio::join!(
        store.confirm_upload(&input.order_id, &upload.id),
        store.confirm_upload(&input.order_id, &upload.id)
    );
    assert_eq!(a.unwrap().revision, 2);
    assert_eq!(b.unwrap().revision, 2);
    let accepted = store
        .create_import_run(&request, spec.clone())
        .await
        .unwrap();
    assert_eq!(
        store.create_import_run(&request, spec).await.unwrap().id,
        accepted.id
    );
    assert_eq!(
        store
            .existing_import_request(&request)
            .await
            .unwrap()
            .unwrap()
            .id,
        accepted.id
    );
    let mut changed = request.clone();
    changed.import.path = "changed.bin".into();
    assert!(matches!(
        store.existing_import_request(&changed).await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        store.order(&input.order_id).await.unwrap().draft,
        draft.draft
    );
    assert_eq!(
        store
            .uploads(&input.order_id, 0, 1)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert!(store.uploads(&input.order_id, 0, 0).await.is_err());
    let export = store.export_order(&input.order_id).await.unwrap();
    assert_eq!(export.format_version, 13);
    assert_eq!(export.data.uploads[0].id, upload.id);
    assert_eq!(export.sha256, canonical_hash(&export.data));
    input.order_id
}

#[tokio::test]
async fn uploads_reservations_confirmation_and_native_import_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uploads.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let order = upload_storage_contract(&store).await;
    let exported = store.export_order(&order).await.unwrap();
    store.close().await;
    let reopened = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        reopened.export_order(&order).await.unwrap().sha256,
        exported.sha256
    );
    reopened.close().await;
}

async fn native_action_storage_contract(store: &Store) {
    let (input, mut spec) = setup(store).await;
    let order = store
        .save_draft(
            &input.order_id,
            SaveDraft {
                base_revision: 1,
                draft: "Keep this draft".into(),
            },
        )
        .await
        .unwrap();
    let request = CreateRestoreRun {
        order_id: order.id.clone(),
        base_revision: order.revision,
        idempotency_key: id(),
        prompt: "Restore retained content".into(),
        access_mode: AccessMode::ConfirmMutations,
        restore: RestoreFile {
            expected_source_sha256: None,
            folder_id: "folder".into(),
            path: "restored".into(),
            source: FileRestoreSource {
                run_id: id(),
                tool_call_id: "original".into(),
                version: RetainedVersion::TextBefore,
            },
        },
    };
    spec.provider = None;
    spec.prompt = request.prompt.clone();
    spec.file_action = Some(ToolCall {
        id: "restore".into(),
        name: "restore_file".into(),
        arguments: json!(request.restore),
    });
    spec.allowed_tools = vec!["restore_file".into()];
    let run = store
        .create_restore_run(&request, spec.clone())
        .await
        .unwrap();
    assert!(run.spec.provider.is_none());
    assert_eq!(
        store
            .stored_run(&run.id)
            .await
            .unwrap()
            .checkpoint
            .pending_tools,
        vec![spec.file_action.clone().unwrap()]
    );
    assert_eq!(
        store.order(&order.id).await.unwrap().draft,
        "Keep this draft"
    );
    assert_eq!(
        store
            .create_restore_run(&request, spec.clone())
            .await
            .unwrap()
            .id,
        run.id
    );
    let mut changed = request.clone();
    changed.restore.path = "other".into();
    assert!(matches!(
        store.existing_restore_request(&changed).await,
        Err(Error::Conflict)
    ));
    let mut agent = input.clone();
    agent.idempotency_key = request.idempotency_key.clone();
    assert!(matches!(
        store.existing_request(&agent).await,
        Err(Error::Conflict)
    ));
    store.cancel(&run.id).await.unwrap();
}

#[tokio::test]
async fn native_action_is_idempotent_and_preserves_the_draft() {
    native_action_storage_contract(&Store::connect("sqlite::memory:").await.unwrap()).await;
}

async fn file_ownership_contract(store: &Store) {
    let (mut input, mut spec) = setup(store).await;
    let folder = FolderGrant {
        order_id: None,
        id: "folder".into(),
        project_id: spec.project_id.clone(),
        name: "Logical storage fixture".into(),
        writable: true,
        revision: hash(b"config"),
        resource_id: id(),
    };
    input.folders = vec![folder.clone()];
    spec.folders = vec![folder];
    let first = store.create_run(&input, spec.clone()).await.unwrap();
    input.base_revision = store.order(&input.order_id).await.unwrap().revision;
    input.idempotency_key = id();
    let second = store.create_run(&input, spec).await.unwrap();
    // Ownership cannot be published as a running operation without a live lease.
    assert!(matches!(
        store
            .mutate(&first.id, None, |r| {
                r.transition(RunState::Running)?;
                let mut change = Change::event("run_started", json!({}));
                change.claim_folders = true;
                Ok(change)
            })
            .await,
        Err(Error::StaleLease)
    ));
    assert_eq!(
        store.stored_run(&first.id).await.unwrap().record.state,
        RunState::Queued
    );
    let own = |r: &mut StoredRun| {
        r.transition(RunState::Running)?;
        r.lease = Some(Lease {
            token: id(),
            until: now() + 15_000,
        });
        r.record.started_at.get_or_insert(now());
        let mut c = Change::event("run_started", json!({}));
        c.claim_folders = true;
        Ok(c)
    };
    store.mutate(&first.id, None, own).await.unwrap();
    assert!(matches!(
        store.mutate(&second.id, None, own).await,
        Err(Error::WorkspaceBusy)
    ));
    assert_eq!(
        store.detail(&second.id).await.unwrap().run.state,
        RunState::Queued
    );
    store
        .mutate(&first.id, None, |r| {
            r.transition(RunState::WaitingApproval)?;
            Ok(Change::event("waiting", json!({})))
        })
        .await
        .unwrap();
    assert!(matches!(
        store.mutate(&second.id, None, own).await,
        Err(Error::WorkspaceBusy)
    ));
    store.cancel(&first.id).await.unwrap();
    store.mutate(&second.id, None, own).await.unwrap();
    let receipt = FileChange {
        run_id: second.id.clone(),
        tool_call_id: "write".into(),
        folder_id: "folder".into(),
        path: "note.txt".into(),
        before_sha256: None,
        after_sha256: hash(b"new"),
        size: 3,
    };
    let before = store.detail(&second.id).await.unwrap().run;
    let failed = store
        .mutate(&second.id, None, |r| {
            r.transition(RunState::Completed)?;
            let mut c = Change::event("run_completed", json!({}));
            c.file_changes = vec![receipt.clone(), receipt.clone()];
            Ok(c)
        })
        .await;
    assert!(failed.is_err());
    assert_eq!(
        store.detail(&second.id).await.unwrap().run.revision,
        before.revision
    );
    assert!(store.file_changes(&second.id).await.unwrap().is_empty());
    let node = FileNode {
        kind: FileNodeKind::File,
        sha256: hash(b"new"),
        size: 3,
        resource_id: "native-file-identity".into(),
    };
    let operation = FileOperation {
        run_id: second.id.clone(),
        tool_call_id: "rename".into(),
        folder_id: "folder".into(),
        kind: FileOperationKind::Rename,
        restored_from: None,
        imported_from: None,
        generated_from: None,
        path: "note.txt".into(),
        destination: Some("renamed.txt".into()),
        before: Some(node.clone()),
        after: Some(node),
        entries: vec![],
    };
    let event_count = store.events(&second.id, 0).await.unwrap().len();
    assert!(
        store
            .mutate(&second.id, None, |r| {
                r.transition(RunState::Completed)?;
                let mut c = Change::event("run_completed", json!({}));
                c.file_changes.push(receipt.clone());
                c.file_operations = vec![operation.clone(), operation.clone()];
                Ok(c)
            })
            .await
            .is_err()
    );
    assert_eq!(
        store.detail(&second.id).await.unwrap().run.revision,
        before.revision
    );
    assert_eq!(
        store.events(&second.id, 0).await.unwrap().len(),
        event_count
    );
    assert!(store.file_changes(&second.id).await.unwrap().is_empty());
    assert!(store.file_operations(&second.id).await.unwrap().is_empty());
    store
        .mutate(&second.id, None, |r| {
            r.transition(RunState::Completed)?;
            let mut c = Change::event("run_completed", json!({}));
            c.file_changes.push(receipt.clone());
            c.file_operations.push(operation.clone());
            Ok(c)
        })
        .await
        .unwrap();
    assert_eq!(store.file_changes(&second.id).await.unwrap().len(), 1);
    assert_eq!(
        serde_json::to_value(store.detail(&second.id).await.unwrap().file_operations).unwrap(),
        json!([operation])
    );
}

#[tokio::test]
async fn file_ownership_and_receipt_are_transactional() {
    file_ownership_contract(&Store::connect("sqlite::memory:").await.unwrap()).await;
}

async fn finish(
    store: &Store,
    input: &CreateRun,
    spec: RunSpec,
    content: &str,
) -> (RunRecord, Message) {
    let run = store.create_run(input, spec).await.unwrap();
    // Claim this fixture directly; an independent test database may retain other queued work.
    store
        .mutate(&run.id, None, |r| {
            r.transition(RunState::Running)?;
            Ok(Change::event("run_started", json!({})))
        })
        .await
        .unwrap();
    let message = Message {
        id: id(),
        order_id: input.order_id.clone(),
        run_id: run.id.clone(),
        role: "assistant".into(),
        content: content.into(),
        created_at: now(),
    };
    store
        .mutate(&run.id, None, |r| {
            r.transition(RunState::Completed)?;
            let mut change = Change::event("run_completed", json!({}));
            change.messages.push(message.clone());
            change.artifacts.push(Artifact {
                id: id(),
                order_id: input.order_id.clone(),
                run_id: run.id.clone(),
                name: "Antwort.txt".into(),
                content: content.into(),
                size: content.len() as u64,
                sha256: hash(content.as_bytes()),
                media_type: "text/plain".into(),
                created_at: now(),
            });
            Ok(change)
        })
        .await
        .unwrap();
    (run, message)
}

async fn history_contract(store: &Store) {
    let (mut input, mut spec) = setup(store).await;
    let project_id = spec.project_id.clone();
    let marker = format!("\u{00c4}PFEL_100% {}", id());
    let (_, first) = finish(store, &input, spec.clone(), &marker).await;
    input.base_revision = store.order(&input.order_id).await.unwrap().revision;
    input.idempotency_key = id();
    input.prompt = "Follow-up".into();
    spec.prompt = input.prompt.clone();
    let (_, second) = finish(store, &input, spec.clone(), "Only in the later history").await;
    let before = store.export_order(&input.order_id).await.unwrap();
    assert_eq!(before.sha256, canonical_hash(&before.data));
    assert_eq!(before.data.messages[1].id, first.id);
    assert_eq!(before.data.messages[3].id, second.id);
    assert_eq!(before.data.artifacts.len(), 2);
    let request = BranchOrder {
        title: "Separate continuation".into(),
        through_message_id: first.id.clone(),
        idempotency_key: id(),
    };
    let branch = store
        .branch_order(&input.order_id, request.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .branch_order(&input.order_id, request.clone())
            .await
            .unwrap()
            .id,
        branch.id
    );
    let mut wrong = request.clone();
    wrong.title = "Changed request".into();
    assert!(matches!(
        store.branch_order(&input.order_id, wrong).await,
        Err(Error::Conflict)
    ));
    let snapshot = store.history(&branch.id).await.unwrap().inherited.unwrap();
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.messages[1].id, first.id);
    assert_eq!(snapshot.artifacts.len(), 1);
    assert_eq!(snapshot.artifacts[0].sha256, hash(marker.as_bytes()));
    assert_eq!(snapshot.artifacts[0].order_id, input.order_id);
    assert_eq!(store.artifacts(&branch.id).await.unwrap().len(), 1);
    assert!(store.runs(&branch.id).await.unwrap().is_empty());
    assert!(store.messages(&branch.id).await.unwrap().is_empty());
    assert_eq!(
        before.sha256,
        store.export_order(&input.order_id).await.unwrap().sha256
    );
    let hits = store
        .search(&marker.to_lowercase(), Some(&project_id), 0, 100)
        .await
        .unwrap();
    assert_eq!(hits.items.len(), 4); // original + branch, each message + artifact
    assert!(
        hits.items
            .iter()
            .all(|h| h.excerpt.contains("\u{00c4}PFEL_100%"))
    );
    assert_eq!(
        store
            .search(&marker, Some("another-project"), 0, 100)
            .await
            .unwrap()
            .items
            .len(),
        0
    );
    assert!(
        store
            .search("%", Some(&project_id), 0, 100)
            .await
            .unwrap()
            .items
            .iter()
            .all(|h| h.excerpt.contains('%'))
    );
    let page = store
        .search(&marker, Some(&project_id), 0, 1)
        .await
        .unwrap();
    assert_eq!(page.next_offset, Some(1));
    assert_ne!(
        page.items[0].id,
        store
            .search(&marker, Some(&project_id), 1, 1)
            .await
            .unwrap()
            .items[0]
            .id
    );
    let count = store.orders(&project_id).await.unwrap().len();
    let invalid = BranchOrder {
        title: "No partial branch".into(),
        through_message_id: before.data.messages[0].id.clone(),
        idempotency_key: id(),
    };
    assert!(matches!(
        store.branch_order(&input.order_id, invalid).await,
        Err(Error::Validation(_))
    ));
    let foreign = BranchOrder {
        title: "No unrelated history".into(),
        through_message_id: id(),
        idempotency_key: id(),
    };
    assert!(matches!(
        store.branch_order(&input.order_id, foreign).await,
        Err(Error::NotFound)
    ));
    assert_eq!(store.orders(&project_id).await.unwrap().len(), count);
    let nested = store
        .branch_order(
            &branch.id,
            BranchOrder {
                title: "Nested branch".into(),
                through_message_id: first.id,
                idempotency_key: id(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .history(&nested.id)
            .await
            .unwrap()
            .inherited
            .unwrap()
            .messages
            .len(),
        2
    );
    let mut next_input = input.clone();
    next_input.order_id = branch.id.clone();
    next_input.base_revision = 1;
    next_input.idempotency_key = id();
    next_input.prompt = "Continue separately".into();
    let mut next_spec = spec.clone();
    next_spec.order_id = branch.id.clone();
    next_spec.prompt = next_input.prompt.clone();
    let next = store.create_run(&next_input, next_spec).await.unwrap();
    let checkpoint = store.stored_run(&next.id).await.unwrap().checkpoint;
    assert_eq!(checkpoint.messages.len(), 4); // system, 2 inherited, new user
    assert_eq!(checkpoint.messages[2]["content"], marker);
    assert!(checkpoint.approvals.is_empty());
    let activity = store.activity(false, 0, 100).await.unwrap();
    let item = activity.items.iter().find(|i| i.run.id == next.id).unwrap();
    assert_eq!(item.order_title, branch.title);
    assert_eq!(item.project_name, "Contract fixture");
    assert!(
        !store
            .activity(true, 0, 100)
            .await
            .unwrap()
            .items
            .iter()
            .any(|i| i.run.id == next.id)
    );
    store
        .mutate(&next.id, None, |r| {
            r.transition(RunState::Running)?;
            Ok(Change::event("run_started", json!({})))
        })
        .await
        .unwrap();
    store
        .mutate(&next.id, None, |r| {
            r.transition(RunState::WaitingInput)?;
            Ok(Change::event("waiting_input", json!({})))
        })
        .await
        .unwrap();
    assert!(
        store
            .activity(true, 0, 100)
            .await
            .unwrap()
            .items
            .iter()
            .any(|i| i.run.id == next.id)
    );
    store.cancel(&next.id).await.unwrap();
    assert!(
        !store
            .activity(false, 0, 100)
            .await
            .unwrap()
            .items
            .iter()
            .any(|i| i.run.id == next.id)
    );
}

#[tokio::test]
async fn history_search_export_and_branch_contract() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    history_contract(&store).await;
    let projects = store.projects().await.unwrap();
    let branch = store
        .orders(&projects[0].id)
        .await
        .unwrap()
        .into_iter()
        .find(|o| o.title == "Separate continuation")
        .unwrap();
    let exported = store.export_order(&branch.id).await.unwrap();
    store.close().await;
    let reopened = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        reopened.export_order(&branch.id).await.unwrap().sha256,
        exported.sha256
    );
    reopened.close().await;
}

#[tokio::test]
async fn concurrent_branch_replays_produce_one_snapshot() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (input, spec) = setup(&store).await;
    let (_, message) = finish(&store, &input, spec, "One immutable snapshot").await;
    let request = BranchOrder {
        title: "One branch".into(),
        through_message_id: message.id,
        idempotency_key: id(),
    };
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..12 {
        let store = store.clone();
        let source = input.order_id.clone();
        let request = request.clone();
        tasks.spawn(async move { store.branch_order(&source, request).await });
    }
    let mut ids = std::collections::HashSet::new();
    while let Some(result) = tasks.join_next().await {
        ids.insert(result.unwrap().unwrap().id);
    }
    assert_eq!(ids.len(), 1);
}

async fn waiting_budget_contract(store: &Store) {
    let (mut input, mut spec) = setup(store).await;
    spec.budgets.max_seconds = 1;
    input.budgets = spec.budgets.clone();
    let run = store.create_run(&input, spec).await.unwrap();
    store
        .mutate(&run.id, None, |r| {
            r.record.started_at = Some(now() - 2000);
            r.transition(RunState::Running)?;
            r.transition(RunState::WaitingInput)?;
            Ok(Change::event("input_requested", json!({})))
        })
        .await
        .unwrap();
    assert!(matches!(
        store
            .submit_input(
                &run.id,
                SubmitInput {
                    request_id: "late".into(),
                    answer: "Too late".into()
                }
            )
            .await,
        Err(Error::BudgetExceeded)
    ));
    assert!(store.expire_waiting_runs().await.unwrap() >= 1);
    let expired = store.stored_run(&run.id).await.unwrap();
    assert_eq!(expired.record.state, RunState::Expired);
    assert_eq!(
        expired.record.error_code.as_deref(),
        Some("budget_exceeded")
    );
    assert_eq!(
        store.events(&run.id, 0).await.unwrap().last().unwrap().kind,
        "run_expired"
    );
}

#[tokio::test]
async fn waiting_budget_expires_transactionally() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    waiting_budget_contract(&store).await;
}
#[tokio::test]
async fn stale_draft_cannot_overwrite_confirmed_changes() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (input, _) = setup(&store).await;
    let saved = store
        .save_draft(
            &input.order_id,
            SaveDraft {
                base_revision: 1,
                draft: "first".into(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .save_draft(
                &input.order_id,
                SaveDraft {
                    base_revision: 1,
                    draft: "stale".into()
                }
            )
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        store.order(&input.order_id).await.unwrap().draft,
        saved.draft
    );
}
#[tokio::test]
async fn a_failed_artifact_insert_rolls_back_state_and_event() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (input, spec) = setup(&store).await;
    let run = store.create_run(&input, spec).await.unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    let token = &claim.lease.as_ref().unwrap().token;
    let before = store.stored_run(&run.id).await.unwrap().record;
    let result = store
        .mutate(&run.id, Some(token), |r| {
            r.transition(RunState::Completed)?;
            let a = Artifact {
                id: "duplicate".into(),
                run_id: r.record.id.clone(),
                order_id: r.record.spec.order_id.clone(),
                name: "x".into(),
                media_type: "text/plain".into(),
                size: 1,
                sha256: hash(b"x"),
                content: "x".into(),
                created_at: now(),
            };
            let mut change = Change::event("completed", json!({}));
            change.artifacts = vec![a.clone(), a];
            Ok(change)
        })
        .await;
    assert!(result.is_err());
    let after = store.stored_run(&run.id).await.unwrap().record;
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.state, RunState::Running);
    assert!(store.artifacts(&input.order_id).await.unwrap().is_empty());
    assert_eq!(store.events(&run.id, 0).await.unwrap().len(), 2);
}
#[tokio::test]
async fn lease_loss_interrupts_without_automatic_replay() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (input, spec) = setup(&store).await;
    let run = store.create_run(&input, spec).await.unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    let token = &claim.lease.as_ref().unwrap().token;
    store
        .mutate(&run.id, Some(token), |r| {
            r.lease.as_mut().unwrap().until = now() - 1;
            r.checkpoint.in_flight = Some(ToolCall {
                id: "unsafe".into(),
                name: "external_write".into(),
                arguments: json!({}),
            });
            Ok(Change::event("started", json!({})))
        })
        .await
        .unwrap();
    assert_eq!(store.recover_expired_leases().await.unwrap(), 1);
    assert_eq!(
        store.stored_run(&run.id).await.unwrap().record.state,
        RunState::Interrupted
    );
    assert!(store.claim().await.unwrap().is_none());
    assert!(matches!(store.resume(&run.id).await, Err(Error::Conflict)));
    assert!(matches!(
        store.heartbeat(&run.id, token).await,
        Err(Error::StaleLease)
    ));
}
#[tokio::test]
async fn cancellation_waits_for_worker_acknowledgement() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (input, spec) = setup(&store).await;
    let run = store.create_run(&input, spec).await.unwrap();
    store.claim().await.unwrap();
    assert_eq!(
        store.cancel(&run.id).await.unwrap().state,
        RunState::CancelRequested
    );
    assert_eq!(
        store.cancel(&run.id).await.unwrap().state,
        RunState::CancelRequested
    );
}

async fn workspace_completion_contract(store: &Store) {
    let (input, spec) = setup(store).await;
    let run = store.create_run(&input, spec).await.unwrap();
    let claimed = store.claim().await.unwrap().unwrap();
    assert_eq!(claimed.record.id, run.id);
    let token = claimed.lease.unwrap().token;
    let mut snapshot = WorkspaceSnapshot {
        run_id: run.id.clone(),
        order_id: input.order_id.clone(),
        entries: vec![WorkspaceEntry {
            path: "file.bin".into(),
            sha256: Some(hash(b"binary")),
            size: 6,
        }],
        sha256: String::new(),
    };
    snapshot.sha256 = workspace_snapshot_hash(&snapshot);
    let message = Message {
        id: id(),
        run_id: run.id.clone(),
        order_id: input.order_id.clone(),
        role: "assistant".into(),
        content: "Snapshot result".into(),
        created_at: now(),
    };
    let before = store.stored_run(&run.id).await.unwrap();
    let invalid = store
        .mutate(&run.id, Some(&token), |r| {
            r.transition(RunState::Completed)?;
            r.lease = None;
            let mut change = Change::event("completed", json!({}));
            change.messages.push(message.clone());
            let mut wrong = snapshot.clone();
            wrong.sha256 = "tampered".into();
            change.workspace_snapshot = Some(wrong);
            Ok(change)
        })
        .await;
    assert!(invalid.is_err());
    assert_eq!(
        store.stored_run(&run.id).await.unwrap().record.revision,
        before.record.revision
    );
    assert!(store.workspace_snapshot(&run.id).await.unwrap().is_none());
    assert_eq!(store.messages(&input.order_id).await.unwrap().len(), 1);
    store
        .mutate(&run.id, Some(&token), |r| {
            r.transition(RunState::Completed)?;
            r.lease = None;
            let mut change = Change::event("completed", json!({}));
            change.messages.push(message.clone());
            change.workspace_snapshot = Some(snapshot.clone());
            Ok(change)
        })
        .await
        .unwrap();
    assert_eq!(
        store.detail(&run.id).await.unwrap().workspace_snapshot,
        Some(snapshot.clone())
    );
    let branch = store
        .branch_order(
            &input.order_id,
            BranchOrder {
                title: "Snapshot branch".into(),
                through_message_id: message.id,
                idempotency_key: id(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.workspace_base(&branch.id).await.unwrap(),
        Some(snapshot.clone())
    );
    let export = store.export_order(&input.order_id).await.unwrap();
    assert_eq!(export.data.runs[0].workspace_snapshot, Some(snapshot));
    assert_eq!(export.sha256, canonical_hash(&export.data));
}

#[tokio::test]
async fn workspace_completion_and_branch_are_one_durable_contract() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    workspace_completion_contract(&store).await;
}
