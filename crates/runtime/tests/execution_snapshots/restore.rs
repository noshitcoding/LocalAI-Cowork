use super::*;
use cowork_workflow_contracts::{ExecutionPreflightV2, PrepareRestoreExecutionPlanV2};

fn database() -> Option<String> {
    let value = std::env::var("COWORK_TEST_RESTORE_DATABASE_URL").ok()?;
    let name = value.rsplit('/').next().unwrap();
    assert!(
        value.contains("@127.0.0.1:19095/")
            && name.strip_prefix("cowork_copy_restore_").is_some_and(
                |suffix| suffix.len() == 32 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
            ),
        "Restore tests only accept an explicit private qualification database"
    );
    Some(value)
}

async fn source_copy() -> (Fixture, RunRecord) {
    let (endpoint, model, server) = model_server().await;
    let mut f = Fixture::database(Some(endpoint), database().as_deref()).await;
    let RunRequest::Single(request) = &mut f.input.request else {
        unreachable!()
    };
    request.access_mode = AccessMode::Autonomous;
    let prepared = f.prepare().await;
    let run = f.commit(&prepared).await.unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f.runtime, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(model.requests.lock().unwrap().len(), 3);
    server.abort();
    let _ = server.await;
    f.runtime.providers.write().await.clear();
    (f, run)
}

async fn restore_request(
    f: &Fixture,
    source: &RunRecord,
    key: &str,
) -> PrepareRestoreExecutionPlanV2 {
    let order = f.runtime.store.order(f.order()).await.unwrap();
    let capture = f
        .runtime
        .prepare_execution_input(
            f.order(),
            &PrepareExecutionInput {
                idempotency_key: format!("{key}-input"),
                base_revision: order.revision,
                folder_ids: vec!["source".into()],
            },
        )
        .await
        .unwrap();
    PrepareRestoreExecutionPlanV2 {
        target: OwnerExecutionTargetV2::BrokerController {},
        request: CreateRestoreRun {
            order_id: order.id,
            base_revision: order.revision,
            idempotency_key: key.into(),
            prompt: "Restore the retained version into a new copy".into(),
            access_mode: AccessMode::ConfirmMutations,
            restore: RestoreFile {
                expected_source_sha256: None,
                folder_id: "source".into(),
                path: "restored.txt".into(),
                source: FileRestoreSource {
                    run_id: source.id.clone(),
                    tool_call_id: "write-copy".into(),
                    version: RetainedVersion::TextAfter,
                },
            },
        },
        input: ExecutionInputRef {
            snapshot_id: capture.snapshot.id,
            sha256: capture.sha256,
            manifest_sha256: capture.snapshot.manifest.sha256,
        },
    }
}

#[tokio::test]
async fn confirmed_restore_is_model_free_preserves_draft_and_both_original_and_prior_copy() {
    let (f, source) = source_copy().await;
    let order = f.runtime.store.order(f.order()).await.unwrap();
    f.runtime
        .store
        .save_draft(
            &order.id,
            SaveDraft {
                base_revision: order.revision,
                draft: "Keep this unrelated draft".into(),
            },
        )
        .await
        .unwrap();
    let request = restore_request(&f, &source, "copy-restore").await;
    let plan = f
        .runtime
        .prepare_restore_execution_plan_v2(&request)
        .await
        .unwrap();
    let ExecutionPreflightV2::Restore { review } = &plan.plan.preflight else {
        panic!("restore review required")
    };
    assert_eq!(review.content.sha256, hash(b"after"));
    assert_eq!(review.restore.expected_source_sha256, Some(hash(b"after")));
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    let run = f.commit(&plan).await.unwrap();
    if std::env::var("COWORK_WRITE_RESTORE_PLAN_FIXTURE").as_deref() == Ok("1") {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/client/src/fixtures/execution-restore-plan-v2.json");
        assert!(
            !path.exists(),
            "Public restore interoperability fixture already exists; do not overwrite it"
        );
        std::fs::write(
            path,
            serde_json::to_string_pretty(&json!({"prepare":request,"prepared":plan,"run":run}))
                .unwrap()
                + "\n",
        )
        .unwrap();
    }
    assert!(run.spec.provider.is_none() && run.spec.input_workspace.is_some());
    assert_ne!(run.spec.input_workspace, source.spec.input_workspace);
    assert_eq!(
        f.runtime.store.order(f.order()).await.unwrap().draft,
        "Keep this unrelated draft"
    );
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f.runtime, &run.id, RunState::WaitingApproval).await;
    stop.cancel();
    worker.await.unwrap();
    assert!(
        f.runtime
            .store
            .file_operations(&run.id)
            .await
            .unwrap()
            .is_empty()
    );
    let approval = f
        .runtime
        .store
        .detail(&run.id)
        .await
        .unwrap()
        .approvals
        .into_iter()
        .find(|a| a.decision.is_none())
        .unwrap();
    f.runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint,
                allow: true,
            },
        )
        .await
        .unwrap();
    let store = if let Some(url) = database() {
        Store::connect(&url).await.unwrap()
    } else {
        Store::sqlite(&f.dir.path().join("snapshot.sqlite"))
            .await
            .unwrap()
    };
    let broker = FileBroker::open(
        &f.dir.path().join("broker"),
        vec![FolderConfig {
            id: "source".into(),
            project_id: source.spec.project_id.clone(),
            name: "Selected source".into(),
            path: f.dir.path().join("originals"),
            writable: true,
        }],
    )
    .unwrap();
    let reopened = Runtime::new(store, vec![], false)
        .unwrap()
        .with_file_broker(broker);
    let stop = CancellationToken::new();
    let worker = tokio::spawn(reopened.clone().serve(1, stop.clone()));
    state(&reopened, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    let detail = reopened.store.detail(&run.id).await.unwrap();
    assert!(detail.run.model_attempts.is_empty());
    assert_eq!(detail.file_operations.len(), 1);
    assert_eq!(detail.file_operations[0].kind, FileOperationKind::Restore);
    let output = reopened
        .download_snapshot(
            f.order(),
            &run.id,
            DownloadFile {
                path: "input-0001/restored.txt".into(),
                expected_sha256: Some(hash(b"after")),
            },
        )
        .await
        .unwrap();
    assert_eq!(output.bytes, b"after");
    let previous = reopened
        .run_copy_text(
            f.order(),
            &source.id,
            FileLocation {
                folder_id: "source".into(),
                path: "note.txt".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(previous.content, "after");
    assert_eq!(
        std::fs::read(f.dir.path().join("originals/note.txt")).unwrap(),
        b"before"
    );
    assert!(!f.dir.path().join("originals/restored.txt").exists());
    let replay = reopened
        .commit_execution_plan_v2(
            f.order(),
            &plan.plan.id,
            &CommitExecutionPlan {
                sha256: plan.sha256,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        serde_json::to_value(run).unwrap()
    );
    reopened.store.close().await;
    drop(reopened);
    f.close().await;
}

#[tokio::test]
async fn restore_review_rejects_foreign_source_wrong_hash_existing_target_and_read_only() {
    let (f, source) = source_copy().await;
    let request = restore_request(&f, &source, "rejected-restore").await;
    let mut wrong = request.clone();
    wrong.request.restore.expected_source_sha256 = Some("0".repeat(64));
    assert!(matches!(
        f.runtime.prepare_restore_execution_plan_v2(&wrong).await,
        Err(Error::Conflict)
    ));
    let mut readonly = request.clone();
    readonly.request.access_mode = AccessMode::ReadOnly;
    assert!(matches!(
        f.runtime.prepare_restore_execution_plan_v2(&readonly).await,
        Err(Error::Forbidden)
    ));
    let mut existing = request.clone();
    existing.request.restore.path = "note.txt".into();
    assert!(matches!(
        f.runtime.prepare_restore_execution_plan_v2(&existing).await,
        Err(Error::Conflict)
    ));
    let mut foreign = request.clone();
    foreign.request.restore.source.run_id = "another-owner-run".into();
    assert!(
        f.runtime
            .prepare_restore_execution_plan_v2(&foreign)
            .await
            .is_err()
    );
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    f.close().await;
}

#[tokio::test]
async fn concurrent_restore_confirmation_has_one_original_receipt_and_lost_prepare_can_retire() {
    let (f, source) = source_copy().await;
    let request = restore_request(&f, &source, "restore-race").await;
    let plan = f
        .runtime
        .prepare_restore_execution_plan_v2(&request)
        .await
        .unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let runtime = f.runtime.clone();
        let order = f.order().to_owned();
        let plan = plan.clone();
        tasks.spawn(async move {
            runtime
                .commit_execution_plan_v2(
                    &order,
                    &plan.plan.id,
                    &CommitExecutionPlan {
                        sha256: plan.sha256,
                    },
                )
                .await
        });
    }
    let mut original = None;
    while let Some(result) = tasks.join_next().await {
        let ack = serde_json::to_value(result.unwrap().unwrap()).unwrap();
        if let Some(old) = &original {
            assert_eq!(old, &ack)
        } else {
            original = Some(ack)
        }
    }
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 2);
    let next = restore_request(&f, &source, "lost-restore-prepare").await;
    f.runtime
        .retire_restore_execution_request_v2(&next)
        .await
        .unwrap();
    assert!(
        f.runtime
            .prepare_restore_execution_plan_v2(&next)
            .await
            .is_err()
    );
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 2);
    f.close().await;
}

#[tokio::test]
async fn restore_retirement_before_input_ack_and_old_owner_custody_are_not_reinterpreted() {
    let (f, source) = source_copy().await;
    let old = serde_json::to_value(&f.input).unwrap();
    let retained: cowork_storage::StoredExecutionRequestV2 =
        serde_json::from_value(old.clone()).unwrap();
    assert_eq!(serde_json::to_value(retained).unwrap(), old);
    let request = restore_request(&f, &source, "retire-before-input-ack").await;
    let retired = f
        .runtime
        .store
        .retire_restore_execution_intent_v2(&request.request)
        .await
        .unwrap();
    assert!(retired.retired);
    assert!(
        f.runtime
            .prepare_restore_execution_plan_v2(&request)
            .await
            .is_err()
    );
    assert!(
        f.runtime
            .store
            .retire_restore_execution_intent_v2(&request.request)
            .await
            .unwrap()
            .retired
    );
    let mut changed = request.request.clone();
    changed.restore.path = "other.txt".into();
    assert!(matches!(
        f.runtime
            .store
            .retire_restore_execution_intent_v2(&changed)
            .await,
        Err(Error::Conflict)
    ));
    let planned = restore_request(&f, &source, "retire-stored-restore").await;
    let plan = f
        .runtime
        .prepare_restore_execution_plan_v2(&planned)
        .await
        .unwrap();
    f.runtime
        .store
        .retire_restore_execution_intent_v2(&planned.request)
        .await
        .unwrap();
    assert!(f.commit(&plan).await.is_err());
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    f.close().await;
}

async fn directory_model(
    axum::extract::State(count): axum::extract::State<Arc<Mutex<usize>>>,
    axum::Json(body): axum::Json<Value>,
) -> String {
    let index = {
        let mut count = count.lock().unwrap();
        let value = *count;
        *count += 1;
        value
    };
    let delta = match index {
        0 => {
            json!({"tool_calls":[{"index":0,"id":"stat-dir","function":{"name":"stat_file","arguments":json!({"folder_id":"source","path":"docs"}).to_string()}}]})
        }
        1 => {
            let value = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|message| message["role"] == "tool")
                .unwrap();
            let value: Value = serde_json::from_str(value["content"].as_str().unwrap()).unwrap();
            let sha = value["node"]["sha256"].as_str().unwrap();
            json!({"tool_calls":[{"index":0,"id":"remove-dir","function":{"name":"remove_file","arguments":json!({"folder_id":"source","path":"docs","expected_sha256":sha}).to_string()}}]})
        }
        _ => json!({"content":"The private copy directory is removed."}),
    };
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"delta":delta,"finish_reason":if index<2{"tool_calls"}else{"stop"}}],"usage":{"prompt_tokens":200,"completion_tokens":20}})
    )
}

#[tokio::test]
async fn retained_directory_is_restored_as_a_new_version_without_changing_original_or_prior_result()
{
    let count = Arc::new(Mutex::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .route(
            "/models",
            axum::routing::get(|| async { axum::Json(json!({"data":[{"id":"fixture"}]})) }),
        )
        .route("/chat/completions", axum::routing::post(directory_model))
        .with_state(count.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut f = Fixture::database(Some(endpoint), database().as_deref()).await;
    std::fs::create_dir_all(f.dir.path().join("originals/docs/nested")).unwrap();
    std::fs::write(
        f.dir.path().join("originals/docs/nested/file.txt"),
        b"retained directory",
    )
    .unwrap();
    let capture = f
        .runtime
        .prepare_execution_input(
            f.order(),
            &PrepareExecutionInput {
                idempotency_key: "directory-input".into(),
                base_revision: 1,
                folder_ids: vec!["source".into()],
            },
        )
        .await
        .unwrap();
    f.input.input = ExecutionInputRef {
        snapshot_id: capture.snapshot.id,
        sha256: capture.sha256,
        manifest_sha256: capture.snapshot.manifest.sha256,
    };
    let RunRequest::Single(request) = &mut f.input.request else {
        unreachable!()
    };
    request.access_mode = AccessMode::Autonomous;
    let plan = f.prepare().await;
    let source = f.commit(&plan).await.unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f.runtime, &source.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(*count.lock().unwrap(), 3);
    server.abort();
    let _ = server.await;
    f.runtime.providers.write().await.clear();
    let operation = f
        .runtime
        .store
        .file_operations(&source.id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(operation.kind, FileOperationKind::Remove);
    let mut request = restore_request(&f, &source, "directory-restore").await;
    request.request.access_mode = AccessMode::Autonomous;
    request.request.restore.path = "docs.restored".into();
    request.request.restore.source.tool_call_id = "remove-dir".into();
    request.request.restore.source.version = RetainedVersion::OperationBefore;
    let plan = f
        .runtime
        .prepare_restore_execution_plan_v2(&request)
        .await
        .unwrap();
    let ExecutionPreflightV2::Restore { review } = &plan.plan.preflight else {
        panic!("restore review")
    };
    assert_eq!(review.content.kind, FileNodeKind::Directory);
    let run = f.commit(&plan).await.unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f.runtime, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    let restored = f
        .runtime
        .run_copy_text(
            f.order(),
            &run.id,
            FileLocation {
                folder_id: "source".into(),
                path: "docs.restored/nested/file.txt".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(restored.content, "retained directory");
    assert_eq!(
        std::fs::read(f.dir.path().join("originals/docs/nested/file.txt")).unwrap(),
        b"retained directory"
    );
    assert!(!f.dir.path().join("originals/docs.restored").exists());
    let prior = f
        .runtime
        .run_copy_files(
            f.order(),
            &source.id,
            FileLocation {
                folder_id: "source".into(),
                path: ".".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        !prior
            .entries
            .iter()
            .any(|entry| entry.name == "docs" || entry.name == "docs.restored")
    );
    assert!(
        f.runtime
            .store
            .detail(&run.id)
            .await
            .unwrap()
            .run
            .model_attempts
            .is_empty()
    );
    f.close().await;
}
