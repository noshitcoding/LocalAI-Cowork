use cowork_broker::FileBroker;
use cowork_contracts::*;
use cowork_domain::{Error, hash};
use cowork_runtime::Runtime;
use cowork_storage::{Store, id};
use tokio_util::sync::CancellationToken;

async fn wait(runtime: &Runtime, run: &str, state: RunState) -> RunDetail {
    for _ in 0..500 {
        let current = runtime.store.detail(run).await.unwrap();
        if current.run.state == state {
            return current;
        }
        assert!(
            !matches!(
                current.run.state,
                RunState::Failed | RunState::Expired | RunState::Interrupted | RunState::Canceled
            ),
            "unexpected: {:?} {:?}",
            current.run.state,
            current.run.error_code
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("run did not reach {state:?}")
}

#[tokio::test]
async fn upload_is_durable_separate_from_import_and_uses_all_permission_modes_without_a_provider() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("state.sqlite3"))
        .await
        .unwrap();
    let runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_file_broker(FileBroker::open(&dir.path().join("files"), vec![]).unwrap());
    let project = runtime
        .store
        .create_project(CreateProject {
            name: "Import test".into(),
        })
        .await
        .unwrap();
    for (mode, allow) in [
        (AccessMode::ReadOnly, false),
        (AccessMode::ConfirmMutations, true),
        (AccessMode::ConfirmMutations, false),
        (AccessMode::ConfirmAll, true),
        (AccessMode::Autonomous, true),
    ] {
        let order = runtime
            .store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Import target".into(),
            })
            .await
            .unwrap();
        let order = runtime
            .store
            .save_draft(
                &order.id,
                SaveDraft {
                    base_revision: order.revision,
                    draft: "Keep my draft".into(),
                },
            )
            .await
            .unwrap();
        let reservation = ReserveUpload {
            idempotency_key: id(),
            file: DownloadMetadata {
                file_name: "Prüfung 📄.bin".into(),
                size: 4,
                sha256: hash(&[0, 255, 128, 42]),
            },
        };
        let upload = runtime
            .reserve_upload(&order.id, &reservation)
            .await
            .unwrap();
        assert!(
            runtime
                .receive_upload(&order.id, &upload.id, b"bad".to_vec())
                .await
                .is_err()
        );
        assert_eq!(
            runtime
                .store
                .upload(&order.id, &upload.id)
                .await
                .unwrap()
                .state,
            UploadState::Reserved
        );
        let ready = runtime
            .receive_upload(&order.id, &upload.id, vec![0, 255, 128, 42])
            .await
            .unwrap();
        assert_eq!(ready.state, UploadState::Ready);
        assert_eq!(
            runtime
                .receive_upload(&order.id, &upload.id, vec![0, 255, 128, 42])
                .await
                .unwrap()
                .revision,
            2
        );
        let workspace = runtime.workspace(&order.id).await.unwrap();
        assert!(
            runtime
                .workspace_stat(&order.id, "result.bin".into())
                .await
                .unwrap()
                .node
                .is_none()
        );
        let request = CreateImportRun {
            order_id: order.id.clone(),
            base_revision: order.revision,
            idempotency_key: id(),
            prompt: "Import verified binary".into(),
            access_mode: mode.clone(),
            import: ImportFile {
                folder_id: workspace.folder.id,
                path: "result.bin".into(),
                upload_id: upload.id.clone(),
                file: ready.file,
                expected_sha256: None,
            },
        };
        if mode == AccessMode::ReadOnly {
            assert!(matches!(
                runtime.create_import_run(&request).await,
                Err(Error::Forbidden)
            ));
            continue;
        }
        let run = runtime.create_import_run(&request).await.unwrap();
        assert!(run.spec.provider.is_none());
        assert_eq!(
            runtime.create_import_run(&request).await.unwrap().id,
            run.id
        );
        let stop = CancellationToken::new();
        let worker = tokio::spawn(runtime.clone().serve(1, stop.clone()));
        if mode != AccessMode::Autonomous {
            let waiting = wait(&runtime, &run.id, RunState::WaitingApproval).await;
            assert!(
                runtime
                    .workspace_stat(&order.id, "result.bin".into())
                    .await
                    .unwrap()
                    .node
                    .is_none()
            );
            let approval = &waiting.approvals[0];
            assert!(
                runtime
                    .store
                    .resolve_approval(
                        &run.id,
                        &approval.id,
                        ResolveApproval {
                            fingerprint: hash(b"wrong"),
                            allow
                        }
                    )
                    .await
                    .is_err()
            );
            runtime
                .store
                .resolve_approval(
                    &run.id,
                    &approval.id,
                    ResolveApproval {
                        fingerprint: approval.fingerprint.clone(),
                        allow,
                    },
                )
                .await
                .unwrap();
        }
        let expected = if allow {
            RunState::Completed
        } else {
            RunState::Failed
        };
        let detail = wait(&runtime, &run.id, expected).await;
        stop.cancel();
        worker.await.unwrap();
        if allow {
            let content = runtime
                .download_workspace(
                    &order.id,
                    DownloadFile {
                        path: "result.bin".into(),
                        expected_sha256: Some(reservation.file.sha256),
                    },
                )
                .await
                .unwrap();
            assert_eq!(content.bytes, [0, 255, 128, 42]);
            assert_eq!(
                detail.file_operations[0].imported_from.as_ref().unwrap().id,
                upload.id
            );
            assert!(detail.workspace_snapshot.is_some());
        } else {
            assert!(detail.file_operations.is_empty());
            assert!(
                runtime
                    .workspace_stat(&order.id, "result.bin".into())
                    .await
                    .unwrap()
                    .node
                    .is_none()
            );
        }
        assert_eq!(
            runtime.store.order(&order.id).await.unwrap().draft,
            order.draft
        );
        assert!(
            !runtime
                .store
                .events(&run.id, 0)
                .await
                .unwrap()
                .iter()
                .any(|e| e.kind.starts_with("model_"))
        );
    }
}

#[tokio::test]
async fn foreign_uploads_changed_metadata_and_transfer_capacity_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_file_broker(FileBroker::open(dir.path(), vec![]).unwrap());
    let one = runtime.begin_upload_transfer().unwrap();
    let two = runtime.begin_upload_transfer().unwrap();
    assert!(matches!(
        runtime.begin_upload_transfer(),
        Err(Error::TransferBusy)
    ));
    drop(one);
    drop(two);
    drop(runtime.begin_upload_transfer().unwrap());
    let project = runtime
        .store
        .create_project(CreateProject {
            name: "Owner".into(),
        })
        .await
        .unwrap();
    let order = runtime
        .store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "A".into(),
        })
        .await
        .unwrap();
    let other = runtime
        .store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "B".into(),
        })
        .await
        .unwrap();
    let upload = runtime
        .reserve_upload(
            &order.id,
            &ReserveUpload {
                idempotency_key: id(),
                file: DownloadMetadata {
                    file_name: "empty.bin".into(),
                    size: 0,
                    sha256: hash(b""),
                },
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        runtime.receive_upload(&other.id, &upload.id, vec![]).await,
        Err(Error::NotFound)
    ));
    runtime
        .receive_upload(&order.id, &upload.id, vec![])
        .await
        .unwrap();
    let workspace = runtime.workspace(&other.id).await.unwrap();
    let mut request = CreateImportRun {
        order_id: other.id,
        base_revision: 1,
        idempotency_key: id(),
        prompt: "Import".into(),
        access_mode: AccessMode::Autonomous,
        import: ImportFile {
            folder_id: workspace.folder.id,
            path: "empty.bin".into(),
            upload_id: upload.id,
            file: upload.file,
            expected_sha256: None,
        },
    };
    assert!(runtime.create_import_run(&request).await.is_err());
    request.order_id = order.id.clone();
    request.import.folder_id = runtime.workspace(&order.id).await.unwrap().folder.id;
    request.import.file.sha256 = hash(b"tampered");
    assert!(matches!(
        runtime.create_import_run(&request).await,
        Err(Error::Forbidden)
    ));
    assert!(runtime.store.runs(&order.id).await.unwrap().is_empty());
}
