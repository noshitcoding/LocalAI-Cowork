//! Custody tests use a deliberately empty selection. Native source authorization
//! and bytes are exercised separately through FileBroker/Runtime/HTTP.
use cowork_contracts::*;
use cowork_domain::{
    canonical_hash,
    execution_inputs::{CapturedExecutionInput, request_hash},
    workspace_transfer,
};
use cowork_storage::Store;
use sqlx::AnyPool;

struct Fixture {
    store: Store,
    pool: AnyPool,
    url: String,
    order: Order,
    _directory: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let url = if let Ok(value) = std::env::var("COWORK_EXECUTION_INPUT_TEST_DB") {
            assert!(
                value
                    .rsplit('/')
                    .next()
                    .is_some_and(|v| v.starts_with("cowork_execution_inputs_")),
                "use a dedicated disposable execution-input database"
            );
            value
        } else {
            format!(
                "sqlite:{}?mode=rwc",
                directory.path().join("inputs.sqlite").display()
            )
        };
        let store = Store::connect(&url).await.unwrap();
        let pool = AnyPool::connect(&url).await.unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Input custody".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id,
                title: "Frozen input".into(),
            })
            .await
            .unwrap();
        Self {
            store,
            pool,
            url,
            order,
            _directory: directory,
        }
    }
    fn request(&self, key: &str) -> PrepareExecutionInput {
        PrepareExecutionInput {
            idempotency_key: key.into(),
            base_revision: self.order.revision,
            folder_ids: vec![],
        }
    }
    fn capture(&self, request: &PrepareExecutionInput) -> CapturedExecutionInput {
        let snapshot = ExecutionInputSnapshot {
            format_version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            order_id: self.order.id.clone(),
            project_id: self.order.project_id.clone(),
            order_revision: request.base_revision,
            request_sha256: request_hash(&self.order.id, request),
            created_at: cowork_storage::now(),
            sources: vec![],
            manifest: workspace_transfer::build(vec![], vec![]).unwrap(),
        };
        CapturedExecutionInput {
            sha256: canonical_hash(&snapshot),
            snapshot,
            origins: vec![],
        }
    }
    async fn close(self) {
        self.pool.close().await;
        self.store.close().await;
    }
}

#[tokio::test]
async fn original_receipt_survives_reopen_retirement_and_order_changes_without_creating_runs() {
    let f = Fixture::new().await;
    let request = f.request("capture-once");
    let capture = f.capture(&request);
    let saved = f
        .store
        .retain_execution_input(&request, &capture)
        .await
        .unwrap();
    assert_eq!(saved.view().sha256, capture.sha256);
    let replay = f
        .store
        .retain_execution_input(&request, &f.capture(&request))
        .await
        .unwrap();
    assert_eq!(saved.view(), replay.view());
    f.store
        .save_draft(
            &f.order.id,
            SaveDraft {
                base_revision: f.order.revision,
                draft: "changed after capture".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.store
            .existing_execution_input(&f.order.id, &request)
            .await
            .unwrap()
            .unwrap()
            .view(),
        saved.view()
    );
    assert!(
        f.store
            .retire_execution_input(&f.order.id, &capture.snapshot.id, &"0".repeat(64))
            .await
            .is_err()
    );
    let retired = f
        .store
        .retire_execution_input(&f.order.id, &capture.snapshot.id, &capture.sha256)
        .await
        .unwrap();
    assert!(retired.retired);
    assert_eq!(
        retired.view(),
        f.store
            .retain_execution_input(&request, &f.capture(&request))
            .await
            .unwrap()
            .view()
    );
    assert_eq!(
        retired.view(),
        f.store
            .retire_execution_input(&f.order.id, &capture.snapshot.id, &capture.sha256)
            .await
            .unwrap()
            .view()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runs WHERE order_id=$1")
            .bind(&f.order.id)
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        0
    );
    assert!(
        f.store
            .execution_input("other-order", &capture.snapshot.id)
            .await
            .is_err()
    );
    f.pool.close().await;
    f.store.close().await;
    let reopened = Store::connect(&f.url).await.unwrap();
    assert_eq!(
        reopened
            .execution_input(&f.order.id, &capture.snapshot.id)
            .await
            .unwrap()
            .view(),
        retired.view()
    );
    reopened.close().await;
    f.close().await;
}

#[tokio::test]
async fn concurrency_converges_on_one_original_capture_and_retirement_is_durable() {
    let f = Fixture::new().await;
    let request = f.request("concurrent-input");
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let store = f.store.clone();
        let request = request.clone();
        let capture = f.capture(&request);
        tasks.push(tokio::spawn(async move {
            store
                .retain_execution_input(&request, &capture)
                .await
                .unwrap()
                .view()
        }));
    }
    let mut views = Vec::new();
    for task in tasks {
        views.push(task.await.unwrap());
    }
    assert!(views.iter().all(|v| v == &views[0]));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM owner_execution_inputs WHERE order_id=$1"
        )
        .bind(&f.order.id)
        .fetch_one(&f.pool)
        .await
        .unwrap(),
        1
    );
    let store = f.store.clone();
    let order = f.order.id.clone();
    let view = views[0].clone();
    let retire = tokio::spawn(async move {
        store
            .retire_execution_input(&order, &view.snapshot.id, &view.sha256)
            .await
            .unwrap()
    });
    let repeated = f
        .store
        .retain_execution_input(&request, &f.capture(&request))
        .await
        .unwrap();
    assert_eq!(repeated.capture.snapshot.id, views[0].snapshot.id);
    retire.await.unwrap();
    assert!(
        f.store
            .existing_execution_input(&f.order.id, &request)
            .await
            .unwrap()
            .unwrap()
            .retired
    );
    f.close().await;
}

#[tokio::test]
async fn stale_order_conflicting_keys_and_corrupted_custody_fail_closed() {
    let f = Fixture::new().await;
    let request = f.request("stale-input");
    let capture = f.capture(&request);
    let updated = f
        .store
        .save_draft(
            &f.order.id,
            SaveDraft {
                base_revision: f.order.revision,
                draft: "new revision".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        f.store
            .retain_execution_input(&request, &capture)
            .await
            .is_err()
    );
    assert!(
        f.store
            .existing_execution_input(&f.order.id, &request)
            .await
            .unwrap()
            .is_none()
    );
    let request = PrepareExecutionInput {
        base_revision: updated.revision,
        ..request
    };
    let mut capture = f.capture(&request);
    let saved = f
        .store
        .retain_execution_input(&request, &capture)
        .await
        .unwrap();
    let conflict = PrepareExecutionInput {
        base_revision: updated.revision + 1,
        ..request.clone()
    };
    assert!(
        f.store
            .existing_execution_input(&f.order.id, &conflict)
            .await
            .is_err()
    );
    capture.snapshot.manifest.total_bytes = 1;
    // Even replacing the private storage checksum cannot make an invalid portable
    // manifest or a mismatched public snapshot hash into an accepted receipt.
    sqlx::query("UPDATE owner_execution_inputs SET payload=$1,custody_sha256=$2 WHERE id=$3")
        .bind(serde_json::to_string(&capture).unwrap())
        .bind(canonical_hash(&capture))
        .bind(&saved.capture.snapshot.id)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        f.store
            .execution_input(&f.order.id, &saved.capture.snapshot.id)
            .await
            .is_err()
    );
    f.close().await;
}
