//! Windows host path readiness fails before a workflow/plan/model is admitted.
#![cfg(windows)]
use cowork_contracts::*;
use cowork_domain::{Error, hash, workflows::WorkflowDefinition};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use cowork_workflow_contracts::{
    OwnerExecutionTargetV2, PrepareOwnerExecutionPlanV2, RunRequest, WorkflowRunRequest,
};
use serde_json::json;
use sqlx::Connection;
use std::{collections::BTreeMap, os::windows::ffi::OsStrExt};

#[tokio::test]
async fn long_runtime_path_is_specific_preflight_error_with_no_run_or_plan() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("path-preflight.sqlite3");
    let store = Store::sqlite(&database).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Document path readiness".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Unstarted document workflow".into(),
        })
        .await
        .unwrap();
    let definition: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-native-documents.json"
    ))
    .unwrap();
    assert!(definition.roles.is_empty());
    let workflow = store
        .create_workflow(&project.id, "long-path-source", &definition)
        .await
        .unwrap();
    let prefix = temp.path().as_os_str().encode_wide().count() + 1;
    let toolbox_root = temp.path().join("x".repeat(206 - prefix));
    let toolbox = cowork_toolbox::Toolbox::open(cowork_toolbox::Config {
        root: toolbox_root.clone(),
        programs: BTreeMap::new(),
        environment: BTreeMap::new(),
        github_token: None,
    })
    .unwrap();
    let availability = toolbox.availability();
    assert!(
        !availability.documents_available
            && !availability.ocr_available
            && !availability.latex_available
            && !availability.office_convert_available
    );
    assert_eq!(availability.document_status, "data_path_too_long");
    let runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_file_broker(
            cowork_broker::FileBroker::open(&temp.path().join("broker"), vec![]).unwrap(),
        )
        .with_toolbox(toolbox.clone());
    let workspace = runtime.workspace(&order.id).await.unwrap();
    let original = temp
        .path()
        .join("broker/workspaces")
        .join(&order.id)
        .join("files/source.txt");
    let bytes = b"Original document; no process may read or publish it.";
    std::fs::write(&original, bytes).unwrap();
    let request: WorkflowRunRequest = serde_json::from_value(json!({"kind":"workflow","order_id":order.id,"base_revision":order.revision,"idempotency_key":"blocked-path-workflow","prompt":"Prepare the declared document workflow.","workflow_id":workflow.id,"version_id":workflow.versions[0].id,"access_mode":"confirm_mutations","budgets":definition.budgets,"inputs":{"source_path":"source.txt","source_sha256":hash(bytes)}})).unwrap();
    let error = runtime.preflight_workflow(&request).await.unwrap_err();
    assert!(
        matches!(error, Error::Validation(message) if message == "workflow_document_runtime_path_too_long")
    );
    let input = runtime
        .prepare_execution_input(
            &order.id,
            &PrepareExecutionInput {
                idempotency_key: "original-path-input".into(),
                base_revision: order.revision,
                folder_ids: vec![workspace.folder.id],
            },
        )
        .await
        .unwrap();
    let original_input = input.clone();
    let error = runtime
        .prepare_execution_plan_v2(&PrepareOwnerExecutionPlanV2 {
            target: OwnerExecutionTargetV2::DocumentEngine {},
            request: RunRequest::Workflow(request),
            input: ExecutionInputRef {
                snapshot_id: input.snapshot.id.clone(),
                sha256: input.sha256.clone(),
                manifest_sha256: input.snapshot.manifest.sha256.clone(),
            },
        })
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Validation(message) if message == "workflow_document_runtime_path_too_long")
    );
    assert!(runtime.store.runs(&order.id).await.unwrap().is_empty());
    assert_eq!(std::fs::read(&original).unwrap(), bytes);
    let retained = runtime
        .store
        .execution_input(&order.id, &input.snapshot.id)
        .await
        .unwrap();
    assert_eq!(retained.capture.sha256, original_input.sha256);
    assert!(!retained.retired);
    assert!(
        std::fs::read_dir(toolbox_root.join("document-stages"))
            .unwrap()
            .next()
            .is_none()
    );
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&database)
        .read_only(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let plans: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM owner_execution_plans_v2")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(plans, 0);
    connection.close().await.unwrap();
    runtime.store.close().await;
}
