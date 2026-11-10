use super::*;
use cowork_workflow_contracts::{ExecutionPreflightV2, PrepareImportExecutionPlanV2};

fn database() -> Option<String> {
    let value = std::env::var("COWORK_TEST_IMPORT_DATABASE_URL").ok()?;
    let name = value.rsplit('/').next().unwrap();
    assert!(
        value.contains("@127.0.0.1:19095/")
            && name
                .strip_prefix("cowork_copy_import_")
                .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())),
        "Import tests require an explicitly isolated local database"
    );
    Some(value)
}
async fn request(
    f: &Fixture,
    key: &str,
    bytes: &[u8],
    path: &str,
    expected: Option<String>,
) -> PrepareImportExecutionPlanV2 {
    let order = f.runtime.store.order(f.order()).await.unwrap();
    let upload = f
        .runtime
        .reserve_upload(
            f.order(),
            &ReserveUpload {
                idempotency_key: format!("{key}-upload"),
                file: DownloadMetadata {
                    file_name: "Prüfung.bin".into(),
                    size: bytes.len() as u64,
                    sha256: hash(bytes),
                },
            },
        )
        .await
        .unwrap();
    let upload = f
        .runtime
        .receive_upload(f.order(), &upload.id, bytes.to_vec())
        .await
        .unwrap();
    let workspace = f.runtime.workspace(f.order()).await.unwrap();
    let input = f
        .runtime
        .prepare_execution_input(
            f.order(),
            &PrepareExecutionInput {
                idempotency_key: format!("{key}-input"),
                base_revision: order.revision,
                folder_ids: vec![workspace.folder.id.clone()],
            },
        )
        .await
        .unwrap();
    PrepareImportExecutionPlanV2 {
        target: OwnerExecutionTargetV2::BrokerController {},
        input: ExecutionInputRef {
            snapshot_id: input.snapshot.id,
            sha256: input.sha256,
            manifest_sha256: input.snapshot.manifest.sha256,
        },
        request: CreateImportRun {
            order_id: order.id,
            base_revision: order.revision,
            idempotency_key: key.into(),
            prompt: "Import into a new independent copy".into(),
            access_mode: AccessMode::ConfirmMutations,
            import: ImportFile {
                folder_id: workspace.folder.id,
                path: path.into(),
                upload_id: upload.id,
                file: upload.file,
                expected_sha256: expected,
            },
        },
    }
}
async fn fixture() -> Fixture {
    Fixture::database(None, database().as_deref()).await
}
async fn finish(f: &Fixture, run: &RunRecord) {
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f.runtime, &run.id, RunState::WaitingApproval).await;
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
    stop.cancel();
    worker.await.unwrap();
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
            project_id: run.spec.project_id.clone(),
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
    reopened.store.close().await;
}

#[tokio::test]
async fn confirmed_import_preserves_draft_originals_upload_and_can_branch_its_result() {
    let f = fixture().await;
    let order = f.runtime.store.order(f.order()).await.unwrap();
    f.runtime
        .store
        .save_draft(
            f.order(),
            SaveDraft {
                base_revision: order.revision,
                draft: "Keep my unrelated draft".into(),
            },
        )
        .await
        .unwrap();
    let bytes = [0, 255, 128, 42];
    let input = request(&f, "copy-import", &bytes, "new.bin", None).await;
    let plan = f
        .runtime
        .prepare_import_execution_plan_v2(&input)
        .await
        .unwrap();
    let ExecutionPreflightV2::Import { review } = &plan.plan.preflight else {
        panic!("import review required")
    };
    assert_eq!(review.upload.file.sha256, hash(&bytes));
    assert_eq!(review.import.file.sha256, hash(&bytes));
    assert!(f.runtime.store.runs(f.order()).await.unwrap().is_empty());
    let run = f.commit(&plan).await.unwrap();
    if std::env::var("COWORK_WRITE_IMPORT_PLAN_FIXTURE").as_deref() == Ok("1") {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/client/src/fixtures/execution-import-plan-v2.json");
        assert!(
            !path.exists(),
            "Public import interoperability fixture already exists; never overwrite it"
        );
        std::fs::write(
            path,
            serde_json::to_string_pretty(&json!({"prepare":input,"prepared":plan,"run":run}))
                .unwrap()
                + "\n",
        )
        .unwrap();
    }
    assert!(run.spec.provider.is_none() && run.spec.input_workspace.is_some());
    assert_eq!(
        f.runtime.store.order(f.order()).await.unwrap().draft,
        "Keep my unrelated draft"
    );
    finish(&f, &run).await;
    let detail = f.runtime.store.detail(&run.id).await.unwrap();
    assert!(detail.run.model_attempts.is_empty());
    assert_eq!(detail.file_operations.len(), 1);
    assert_eq!(detail.file_operations[0].kind, FileOperationKind::Import);
    assert!(
        f.runtime
            .workspace_stat(f.order(), "new.bin".into())
            .await
            .unwrap()
            .node
            .is_none()
    );
    assert_eq!(
        std::fs::read(f.dir.path().join("originals/note.txt")).unwrap(),
        b"before"
    );
    let download = f
        .runtime
        .download_snapshot(
            f.order(),
            &run.id,
            DownloadFile {
                path: "input-0001/new.bin".into(),
                expected_sha256: Some(hash(&bytes)),
            },
        )
        .await
        .unwrap();
    assert_eq!(download.bytes, bytes);
    assert_eq!(
        f.runtime
            .store
            .upload(f.order(), &input.request.import.upload_id)
            .await
            .unwrap()
            .file
            .sha256,
        hash(&bytes)
    );
    let answer = f
        .runtime
        .store
        .messages(f.order())
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.run_id == run.id && m.role == "assistant")
        .unwrap();
    let branch = f
        .runtime
        .store
        .branch_order(
            f.order(),
            BranchOrder {
                title: "Use imported result".into(),
                through_message_id: answer.id,
                idempotency_key: "branch-imported".into(),
            },
        )
        .await
        .unwrap();
    let copied = f
        .runtime
        .workspace_stat(&branch.id, "input-0001/new.bin".into())
        .await
        .unwrap();
    assert_eq!(copied.node.unwrap().sha256, hash(&bytes));
    assert_ne!(
        f.runtime
            .workspace(&branch.id)
            .await
            .unwrap()
            .folder
            .resource_id,
        run.spec.folders[0].resource_id
    );
    let replay = f.commit(&plan).await.unwrap();
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        serde_json::to_value(run).unwrap()
    );
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    f.close().await;
}

#[tokio::test]
async fn import_review_rejects_read_only_wrong_upload_path_and_unobserved_replacement() {
    let f = fixture().await;
    let input = request(&f, "invalid-copy-import", b"new", "new.txt", None).await;
    let mut wrong = input.clone();
    wrong.request.access_mode = AccessMode::ReadOnly;
    assert!(matches!(
        f.runtime.prepare_import_execution_plan_v2(&wrong).await,
        Err(Error::Forbidden)
    ));
    wrong = input.clone();
    wrong.request.import.file.sha256 = "0".repeat(64);
    assert!(matches!(
        f.runtime.prepare_import_execution_plan_v2(&wrong).await,
        Err(Error::Forbidden)
    ));
    wrong = input.clone();
    wrong.request.import.expected_sha256 = Some(hash(b"absent"));
    assert!(matches!(
        f.runtime.prepare_import_execution_plan_v2(&wrong).await,
        Err(Error::Conflict)
    ));
    wrong = input.clone();
    wrong.request.import.path = "../escape".into();
    assert!(
        f.runtime
            .prepare_import_execution_plan_v2(&wrong)
            .await
            .is_err()
    );
    wrong = input.clone();
    wrong.request.import.path = "missing/new.txt".into();
    assert!(matches!(
        f.runtime.prepare_import_execution_plan_v2(&wrong).await,
        Err(Error::NotFound)
    ));
    let other = f
        .runtime
        .store
        .create_order(CreateOrder {
            project_id: f.runtime.store.order(f.order()).await.unwrap().project_id,
            title: "Foreign upload owner".into(),
        })
        .await
        .unwrap();
    let upload = f
        .runtime
        .reserve_upload(
            &other.id,
            &ReserveUpload {
                idempotency_key: "foreign".into(),
                file: input.request.import.file.clone(),
            },
        )
        .await
        .unwrap();
    f.runtime
        .receive_upload(&other.id, &upload.id, b"new".to_vec())
        .await
        .unwrap();
    wrong = input.clone();
    wrong.request.import.upload_id = upload.id;
    assert!(
        f.runtime
            .prepare_import_execution_plan_v2(&wrong)
            .await
            .is_err()
    );
    assert!(f.runtime.store.runs(f.order()).await.unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
async fn concurrent_import_commit_has_one_receipt_and_early_retirement_fences_lost_input() {
    let f = fixture().await;
    let input = request(&f, "concurrent-copy-import", b"one", "one.txt", None).await;
    let plan = f
        .runtime
        .prepare_import_execution_plan_v2(&input)
        .await
        .unwrap();
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let runtime = f.runtime.clone();
        let p = plan.clone();
        let order = f.order().to_owned();
        tasks.push(tokio::spawn(async move {
            runtime
                .commit_execution_plan_v2(
                    &order,
                    &p.plan.id,
                    &CommitExecutionPlan { sha256: p.sha256 },
                )
                .await
                .unwrap()
        }));
    }
    let mut receipt = None;
    for task in tasks {
        let run = task.await.unwrap();
        let value = serde_json::to_value(run).unwrap();
        if let Some(original) = &receipt {
            assert_eq!(original, &value);
        } else {
            receipt = Some(value);
        }
    }
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    let retired = request(&f, "retire-before-input-ack", b"one", "one.txt", None).await;
    let fence = f
        .runtime
        .store
        .retire_import_execution_intent_v2(&retired.request)
        .await
        .unwrap();
    assert!(fence.retired);
    assert!(
        f.runtime
            .prepare_import_execution_plan_v2(&retired)
            .await
            .is_err()
    );
    let lost = request(&f, "lost-prepare-ack", b"one", "one.txt", None).await;
    let old = f
        .runtime
        .prepare_import_execution_plan_v2(&lost)
        .await
        .unwrap();
    f.runtime
        .retire_import_execution_request_v2(&lost)
        .await
        .unwrap();
    assert!(f.commit(&old).await.is_err());
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    f.close().await;
}

#[tokio::test]
async fn import_replacement_changes_only_the_confirmed_copy() {
    let f = fixture().await;
    let seed = request(&f, "seed-native-input", b"old", "existing.txt", None).await;
    let mut native = seed.request;
    native.access_mode = AccessMode::Autonomous;
    let original = f.runtime.create_import_run(&native).await.unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f.runtime, &original.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    let original_snapshot = f
        .runtime
        .store
        .detail(&original.id)
        .await
        .unwrap()
        .workspace_snapshot
        .unwrap();
    let input = request(
        &f,
        "replace-in-copy",
        b"new",
        "existing.txt",
        Some(hash(b"old")),
    )
    .await;
    let mut unobserved = input.clone();
    unobserved.request.import.expected_sha256 = None;
    assert!(matches!(
        f.runtime
            .prepare_import_execution_plan_v2(&unobserved)
            .await,
        Err(Error::Conflict)
    ));
    unobserved = input.clone();
    unobserved.request.import.expected_sha256 = Some(hash(b"wrong"));
    assert!(matches!(
        f.runtime
            .prepare_import_execution_plan_v2(&unobserved)
            .await,
        Err(Error::Conflict)
    ));
    let plan = f
        .runtime
        .prepare_import_execution_plan_v2(&input)
        .await
        .unwrap();
    let run = f.commit(&plan).await.unwrap();
    finish(&f, &run).await;
    assert_eq!(
        f.runtime
            .workspace_text(f.order(), "existing.txt".into())
            .await
            .unwrap()
            .content,
        "old"
    );
    assert_eq!(
        f.runtime
            .run_copy_text(
                f.order(),
                &run.id,
                FileLocation {
                    folder_id: input.request.import.folder_id,
                    path: "existing.txt".into()
                }
            )
            .await
            .unwrap()
            .content,
        "new"
    );
    assert_eq!(
        f.runtime
            .store
            .detail(&original.id)
            .await
            .unwrap()
            .workspace_snapshot
            .unwrap()
            .sha256,
        original_snapshot.sha256
    );
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 2);
    f.close().await;
}

#[tokio::test]
async fn corrupted_staged_upload_is_rechecked_before_any_copy_dispatch() {
    let f = fixture().await;
    let bytes = b"verified upload";
    let input = request(&f, "corrupted-upload", bytes, "verified.txt", None).await;
    let plan = f
        .runtime
        .prepare_import_execution_plan_v2(&input)
        .await
        .unwrap();
    let blob = f.dir.path().join("broker/versions").join(hash(bytes));
    assert!(blob.is_file());
    std::fs::write(&blob, b"corrupt").unwrap();
    assert!(f.commit(&plan).await.is_err());
    assert!(f.runtime.store.runs(f.order()).await.unwrap().is_empty());
    let custody: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM owner_run_workspaces WHERE plan_id=$1")
            .bind(&plan.plan.id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(custody, 0);
    std::fs::write(&blob, bytes).unwrap();
    let run = f.commit(&plan).await.unwrap();
    assert!(run.spec.provider.is_none());
    assert_eq!(f.runtime.store.runs(f.order()).await.unwrap().len(), 1);
    f.close().await;
}
