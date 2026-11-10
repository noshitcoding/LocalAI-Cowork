use super::*;
use cowork_workflow_contracts::{ExecutionPreflightV2, RunPreflight, WorkflowRunRequest};

fn database() -> Option<String> {
    let value = std::env::var("COWORK_TEST_WORKFLOW_INPUT_SOURCE_DATABASE_URL").ok()?;
    let name = value.rsplit('/').next().unwrap();
    assert!(
        value.contains("@127.0.0.1:19095/")
            && name
                .strip_prefix("cowork_workflow_input_")
                .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    );
    Some(value)
}
async fn fixture() -> Fixture {
    Fixture::database(None, database().as_deref()).await
}
async fn finish(f: &Fixture, run: &RunRecord) {
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    state(&f.runtime, &run.id, RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
}
async fn source(f: &Fixture) -> (RunRecord, WorkflowRunRequest) {
    let definition = serde_json::from_str::<cowork_domain::workflows::WorkflowDefinition>(
        include_str!("../../../../tests/fixtures/workflow-control.json"),
    )
    .unwrap();
    let order = f.runtime.store.order(f.order()).await.unwrap();
    let workflow = f
        .runtime
        .store
        .create_workflow(&order.project_id, "source-control", &definition)
        .await
        .unwrap();
    let mut request:WorkflowRunRequest=serde_json::from_value(json!({"kind":"workflow","order_id":order.id,"base_revision":order.revision,
        "idempotency_key":"source-workflow","prompt":"Keep the exact native input","workflow_id":workflow.id,"version_id":workflow.versions[0].id,
        "inputs":{"amount":9007199254740993_u64},"budgets":definition.budgets})).unwrap();
    let RunRequest::Single(single) = &f.input.request else {
        panic!()
    };
    request.folders = single.folders.clone();
    let mut prepared = f.input.clone();
    prepared.request = RunRequest::Workflow(request.clone());
    let plan = f
        .runtime
        .prepare_execution_plan_v2(&prepared)
        .await
        .unwrap();
    let run = f.commit(&plan).await.unwrap();
    finish(f, &run).await;
    (
        f.runtime.store.stored_run(&run.id).await.unwrap().record,
        request,
    )
}
async fn target(
    f: &Fixture,
    run: &RunRecord,
    template: &WorkflowRunRequest,
) -> PrepareOwnerExecutionPlanV2 {
    let order = f
        .runtime
        .store
        .create_order(CreateOrder {
            project_id: run.spec.project_id.clone(),
            title: "Reuse exact input".into(),
        })
        .await
        .unwrap();
    let frozen = f
        .runtime
        .prepare_execution_input(
            &order.id,
            &PrepareExecutionInput {
                idempotency_key: "new-native-input".into(),
                base_revision: order.revision,
                folder_ids: vec!["source".into()],
            },
        )
        .await
        .unwrap();
    let mut request = template.clone();
    request.order_id = order.id;
    request.base_revision = order.revision;
    request.idempotency_key = "source-reuse".into();
    request.inputs = None;
    request.input_source = Some(
        f.runtime
            .workflow_input_snapshot(&run.id)
            .await
            .unwrap()
            .source,
    );
    PrepareOwnerExecutionPlanV2 {
        target: OwnerExecutionTargetV2::BrokerController {},
        request: RunRequest::Workflow(request),
        input: ExecutionInputRef {
            snapshot_id: frozen.snapshot.id,
            sha256: frozen.sha256,
            manifest_sha256: frozen.snapshot.manifest.sha256,
        },
    }
}
fn workflow(input: &mut PrepareOwnerExecutionPlanV2) -> &mut WorkflowRunRequest {
    let RunRequest::Workflow(r) = &mut input.request else {
        panic!()
    };
    r
}

#[tokio::test]
async fn original_native_integer_survives_review_commit_worker_restart_and_receipt_replay() {
    let f = fixture().await;
    let (original, template) = source(&f).await;
    let input = target(&f, &original, &template).await;
    let snapshot = f
        .runtime
        .workflow_input_snapshot(&original.id)
        .await
        .unwrap();
    assert_eq!(snapshot.canonical_json, "{\"amount\":9007199254740993}");
    assert_eq!(
        snapshot.source.sha256,
        hash(snapshot.canonical_json.as_bytes())
    );
    let plan = f.runtime.prepare_execution_plan_v2(&input).await.unwrap();
    let ExecutionPreflightV2::Owner {
        review: RunPreflight::Workflow(review),
    } = &plan.plan.preflight
    else {
        panic!()
    };
    let RunRequest::Workflow(request) = &input.request else {
        panic!()
    };
    assert_eq!(review.input_source, request.input_source);
    assert_eq!(
        review.workflow.inputs.as_ref().unwrap()["amount"].as_u64(),
        Some(9007199254740993)
    );
    assert!(plan.canonical_json.contains("9007199254740993"));
    let run = f
        .runtime
        .commit_execution_plan_v2(
            &plan.plan.order_id,
            &plan.plan.id,
            &CommitExecutionPlan {
                sha256: plan.sha256.clone(),
            },
        )
        .await
        .unwrap();
    if let Ok(path) = std::env::var("COWORK_TEST_WORKFLOW_INPUT_SOURCE_ARTIFACT") {
        let path = std::path::PathBuf::from(path);
        assert!(!path.exists());
        std::fs::write(
            path,
            serde_json::to_string_pretty(
                &json!({"prepare":input,"prepared":plan,"run":run,"snapshot":snapshot}),
            )
            .unwrap(),
        )
        .unwrap();
    }
    assert_eq!(
        run.spec.workflow.as_ref().unwrap().inputs.as_ref().unwrap()["amount"].as_u64(),
        Some(9007199254740993)
    );
    assert_ne!(run.spec.input_workspace, original.spec.input_workspace);
    finish(&f, &run).await;
    let current = f.runtime.store.stored_run(&run.id).await.unwrap().record;
    assert!(current.model_attempts.is_empty());
    assert_eq!(
        current
            .spec
            .workflow
            .as_ref()
            .unwrap()
            .inputs
            .as_ref()
            .unwrap()["amount"]
            .as_u64(),
        Some(9007199254740993)
    );
    let url = database().unwrap_or_else(|| {
        format!(
            "sqlite:{}?mode=rwc",
            f.dir.path().join("snapshot.sqlite").display()
        )
    });
    let reopened = Runtime::new(Store::connect(&url).await.unwrap(), vec![], false).unwrap();
    let replay = reopened
        .commit_execution_plan_v2(
            &plan.plan.order_id,
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
    assert_eq!(
        f.runtime
            .store
            .runs(&plan.plan.order_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.runtime
            .store
            .stored_run(&original.id)
            .await
            .unwrap()
            .record
            .spec
            .workflow,
        original.spec.workflow
    );
    reopened.store.close().await;
    f.close().await;
}

#[tokio::test]
async fn input_reference_rejects_other_project_version_hash_ambiguous_input_and_legacy_start() {
    let f = fixture().await;
    let (original, template) = source(&f).await;
    let input = target(&f, &original, &template).await;
    for index in 0..5 {
        let mut invalid = input.clone();
        let request = workflow(&mut invalid);
        match index {
            0 => request.input_source.as_mut().unwrap().sha256 = "0".repeat(64),
            1 => request.workflow_id = "foreign".into(),
            2 => request.version_id = "different".into(),
            3 => request.inputs = Some(Box::new(json!({"amount":9007199254740992_u64}))),
            _ => request.expected_binding = original.spec.workflow.clone(),
        }
        assert!(f.runtime.prepare_execution_plan_v2(&invalid).await.is_err());
    }
    let RunRequest::Workflow(request) = &input.request else {
        panic!()
    };
    assert!(
        matches!(f.runtime.preflight_workflow(request).await,Err(Error::Validation(code)) if code=="workflow_input_source_requires_plan_v2")
    );
    let foreign = f
        .runtime
        .store
        .create_project(CreateProject {
            name: "Foreign source scope".into(),
        })
        .await
        .unwrap();
    let order = f
        .runtime
        .store
        .create_order(CreateOrder {
            project_id: foreign.id,
            title: "Foreign target".into(),
        })
        .await
        .unwrap();
    let workspace = f.runtime.workspace(&order.id).await.unwrap();
    let captured = f
        .runtime
        .prepare_execution_input(
            &order.id,
            &PrepareExecutionInput {
                idempotency_key: "foreign-input".into(),
                base_revision: order.revision,
                folder_ids: vec![workspace.folder.id],
            },
        )
        .await
        .unwrap();
    let mut invalid = input.clone();
    workflow(&mut invalid).order_id = order.id;
    invalid.input = ExecutionInputRef {
        snapshot_id: captured.snapshot.id,
        sha256: captured.sha256,
        manifest_sha256: captured.snapshot.manifest.sha256,
    };
    assert!(
        matches!(f.runtime.prepare_execution_plan_v2(&invalid).await,Err(Error::Validation(code)) if code=="workflow_input_source_mismatch")
    );
    assert!(
        f.runtime
            .store
            .runs(&request.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    f.close().await;
}

#[tokio::test]
async fn original_input_is_rechecked_before_any_copy_or_dispatch_and_repair_keeps_same_plan() {
    let f = fixture().await;
    let (original, template) = source(&f).await;
    let input = target(&f, &original, &template).await;
    let plan = f.runtime.prepare_execution_plan_v2(&input).await.unwrap();
    let payload: String = sqlx::query_scalar("SELECT payload FROM runs WHERE id=$1")
        .bind(&original.id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let mut corrupt: Value = serde_json::from_str(&payload).unwrap();
    corrupt["record"]["spec"]["workflow"]["inputs"]["amount"] = json!(9007199254740992_u64);
    sqlx::query("UPDATE runs SET payload=$1 WHERE id=$2")
        .bind(corrupt.to_string())
        .bind(&original.id)
        .execute(&f.pool)
        .await
        .unwrap();
    let commit = CommitExecutionPlan {
        sha256: plan.sha256.clone(),
    };
    assert!(
        f.runtime
            .commit_execution_plan_v2(&plan.plan.order_id, &plan.plan.id, &commit)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_run_workspaces WHERE plan_id=$1")
            .bind(&plan.plan.id)
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        0
    );
    assert!(
        f.runtime
            .store
            .runs(&plan.plan.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("UPDATE runs SET payload=$1 WHERE id=$2")
        .bind(payload)
        .bind(&original.id)
        .execute(&f.pool)
        .await
        .unwrap();
    let run = f
        .runtime
        .commit_execution_plan_v2(&plan.plan.order_id, &plan.plan.id, &commit)
        .await
        .unwrap();
    finish(&f, &run).await;
    f.close().await;
}
