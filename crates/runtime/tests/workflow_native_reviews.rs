//! Real V2 plan, queue worker and human continuation; no model or document engine.
use cowork_contracts::*;
use cowork_domain::{canonical_hash, workflows::*};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use cowork_workflow_contracts::*;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn values() -> Value {
    json!({"invoice":"INV-RUNTIME-42","issued":"2026-11-01","net_minor":10000,"tax_minor":1900,"gross_minor":11900})
}
struct Fixture {
    temp: tempfile::TempDir,
    runtime: Runtime,
    project: String,
    run: RunRecord,
    plan: PreparedExecutionPlanV2,
}
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&temp.path().join("review.sqlite3"))
            .await
            .unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Native review without providers".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Check an invoice".into(),
            })
            .await
            .unwrap();
        let definition: WorkflowDefinition = serde_json::from_str(include_str!(
            "../../../tests/fixtures/workflow-native-review.json"
        ))
        .unwrap();
        let workflow = store
            .create_workflow(&project.id, "native-review", &definition)
            .await
            .unwrap();
        let runtime = Runtime::new(store, vec![], false)
            .unwrap()
            .with_file_broker(
                cowork_broker::FileBroker::open(&temp.path().join("broker"), vec![]).unwrap(),
            );
        let workspace = runtime.workspace(&order.id).await.unwrap();
        let frozen = runtime
            .prepare_execution_input(
                &order.id,
                &PrepareExecutionInput {
                    idempotency_key: "native-review-input".into(),
                    base_revision: order.revision,
                    folder_ids: vec![workspace.folder.id],
                },
            )
            .await
            .unwrap();
        let initial = json!({"invoice":null,"issued":null,"net_minor":null,"tax_minor":null,"gross_minor":null});
        let request:WorkflowRunRequest=serde_json::from_value(json!({"kind":"workflow","order_id":order.id,"base_revision":order.revision,"idempotency_key":"native-review-start","prompt":"Ask the user to verify the fields.","workflow_id":workflow.id,"version_id":workflow.versions[0].id,"access_mode":"confirm_mutations","budgets":definition.budgets,"inputs":initial})).unwrap();
        let plan = runtime
            .prepare_execution_plan_v2(&PrepareOwnerExecutionPlanV2 {
                target: OwnerExecutionTargetV2::BrokerController {},
                request: RunRequest::Workflow(request),
                input: ExecutionInputRef {
                    snapshot_id: frozen.snapshot.id,
                    sha256: frozen.sha256,
                    manifest_sha256: frozen.snapshot.manifest.sha256,
                },
            })
            .await
            .unwrap();
        let run = runtime
            .commit_execution_plan_v2(
                &order.id,
                &plan.plan.id,
                &CommitExecutionPlan {
                    sha256: plan.sha256.clone(),
                },
            )
            .await
            .unwrap();
        Self {
            temp,
            runtime,
            project: project.id,
            run,
            plan,
        }
    }
    fn worker(&self) -> (CancellationToken, tokio::task::JoinHandle<()>) {
        let stop = CancellationToken::new();
        let worker = tokio::spawn(self.runtime.clone().serve(1, stop.clone()));
        (stop, worker)
    }
    async fn wait_input(&self) -> WorkflowInputView {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let view = self
                .runtime
                .store
                .workflow_run_view(&self.project, &self.run.id)
                .await
                .unwrap();
            if let Some(input) = view.inputs.into_iter().find(|input| input.actionable) {
                return input;
            }
            assert!(
                !matches!(
                    view.run.state,
                    RunState::Failed
                        | RunState::Interrupted
                        | RunState::Expired
                        | RunState::Canceled
                ),
                "{:?}",
                view.run
            );
            assert!(
                tokio::time::Instant::now() < deadline,
                "Native review never became actionable"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    async fn intent(&self, input: &WorkflowInputView) -> WorkflowInputAnswerIntent {
        let initial = self
            .runtime
            .store
            .workflow_input_draft(&self.run.id, &input.request_id)
            .await
            .unwrap();
        let WorkflowInputDraftSave::Saved { draft, .. } = self
            .runtime
            .store
            .save_workflow_input_draft(
                &self.run.id,
                &input.request_id,
                &SaveWorkflowInputDraft {
                    fingerprint: input.fingerprint.clone(),
                    base_revision: initial.draft.revision,
                    idempotency_key: "save-verified-fields".into(),
                    answer: values().to_string(),
                },
            )
            .await
            .unwrap()
        else {
            panic!("saved fields")
        };
        self.runtime
            .store
            .prepare_workflow_input_answer(
                &self.run.id,
                &input.request_id,
                &PrepareWorkflowInputAnswer {
                    fingerprint: input.fingerprint.clone(),
                    base_draft_revision: draft.revision,
                    idempotency_key: "freeze-verified-fields".into(),
                },
            )
            .await
            .unwrap()
    }
    async fn completed(&self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let detail = self.runtime.store.detail(&self.run.id).await.unwrap();
            if detail.run.state == RunState::Completed {
                assert!(detail.file_operations.is_empty() && detail.approvals.is_empty());
                break;
            }
            assert!(
                matches!(
                    detail.run.state,
                    RunState::Queued | RunState::Running | RunState::WaitingForSnapshot
                ),
                "{:?}",
                detail.run
            );
            assert!(
                tokio::time::Instant::now() < deadline,
                "Native review did not complete"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let view = self
            .runtime
            .store
            .workflow_run_view(&self.project, &self.run.id)
            .await
            .unwrap();
        assert!(view.roles.is_empty());
        assert_eq!(view.charged_tokens, 0);
        let stored = self.runtime.store.stored_run(&self.run.id).await.unwrap();
        assert!(stored.record.model_attempts.is_empty());
        let graph = self
            .runtime
            .store
            .stored_workflow(&self.project, &self.run.id)
            .await
            .unwrap();
        assert_eq!(graph.progress.attempts().len(), 2);
        assert_eq!(graph.inputs.len(), 1);
        assert_eq!(graph.agents["review"].turns.len(), 1);
        let WorkflowNodeState::Completed { output, .. } = &graph.progress.nodes()["review"].state
        else {
            panic!("review output")
        };
        assert_eq!(serde_json::from_str::<Value>(output).unwrap(), values());
        assert_eq!(stored.record.spec.workflow, self.run.spec.workflow);
    }
}

#[tokio::test]
async fn actual_v2_worker_waits_for_explicit_typed_answer_and_completes_without_model_or_file_effect()
 {
    let f = Box::pin(Fixture::new()).await;
    let (stop, worker) = f.worker();
    let input = f.wait_input().await;
    assert!(input.review.is_some() && input.role_id.is_none());
    let intent = f.intent(&input).await;
    let accepted = f
        .runtime
        .store
        .commit_workflow_input_answer_intent(&f.run.id, &intent.id)
        .await
        .unwrap();
    assert_eq!(accepted.state, WorkflowInputAnswerIntentState::Accepted);
    f.completed().await;
    let replay = f
        .runtime
        .store
        .commit_workflow_input_answer_intent(&f.run.id, &intent.id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&accepted), canonical_hash(&replay));
    stop.cancel();
    worker.await.unwrap();
    f.runtime.store.close().await;
}

#[tokio::test]
async fn waiting_native_review_restarts_and_explicitly_resumes_the_original_question_and_intent() {
    let mut f = Box::pin(Fixture::new()).await;
    let (stop, worker) = f.worker();
    let input = f.wait_input().await;
    let intent = f.intent(&input).await;
    let original = f
        .runtime
        .store
        .stored_workflow(&f.project, &f.run.id)
        .await
        .unwrap();
    let plan_hash = canonical_hash(&original.plan);
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(
        f.runtime
            .store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .record
            .state,
        RunState::Interrupted
    );
    f.runtime.store.close().await;
    f.runtime = Runtime::new(
        Store::sqlite(&f.temp.path().join("review.sqlite3"))
            .await
            .unwrap(),
        vec![],
        false,
    )
    .unwrap()
    .with_file_broker(
        cowork_broker::FileBroker::open(&f.temp.path().join("broker"), vec![]).unwrap(),
    );
    let view = f
        .runtime
        .store
        .workflow_run_view(&f.project, &f.run.id)
        .await
        .unwrap();
    let preview = view.input_resume_preview;
    assert!(preview.available, "{:?}", preview.blocked_reason);
    let request = ResumeWorkflowInput {
        run_id: f.run.id.clone(),
        base_run_revision: preview.base_run_revision,
        base_workflow_revision: preview.base_workflow_revision,
        expected_checkpoint_sha256: preview.expected_checkpoint_sha256,
        expected_cursor_sha256: preview.expected_cursor_sha256.unwrap(),
        idempotency_key: "explicit-original-review-resume".into(),
    };
    let operation = f
        .runtime
        .store
        .prepare_workflow_operation(
            &f.project,
            &PrepareWorkflowOperation {
                intent_id: "explicit-original-review-resume".into(),
                request: WorkflowOperationRequest::ResumeInput { request },
                draft_snapshot: None,
            },
        )
        .await
        .unwrap();
    let accepted = f
        .runtime
        .commit_workflow_operation(&f.project, &operation.id)
        .await
        .unwrap();
    assert_eq!(accepted.state, WorkflowOperationState::Accepted);
    let (stop, worker) = f.worker();
    let resumed = f.wait_input().await;
    assert_eq!(resumed.request_id, input.request_id);
    assert_eq!(resumed.fingerprint, input.fingerprint);
    assert_eq!(resumed.expires_at, input.expires_at);
    assert_eq!(resumed.review, input.review);
    assert_eq!(
        canonical_hash(
            &f.runtime
                .store
                .workflow_input_answer_intent(&f.run.id, &intent.id)
                .await
                .unwrap()
        ),
        canonical_hash(&intent)
    );
    f.runtime
        .store
        .commit_workflow_input_answer_intent(&f.run.id, &intent.id)
        .await
        .unwrap();
    f.completed().await;
    assert_eq!(
        canonical_hash(
            &f.runtime
                .store
                .stored_workflow(&f.project, &f.run.id)
                .await
                .unwrap()
                .plan
        ),
        plan_hash
    );
    assert_eq!(
        f.runtime
            .commit_execution_plan_v2(
                &f.plan.plan.order_id,
                &f.plan.plan.id,
                &CommitExecutionPlan {
                    sha256: f.plan.sha256.clone()
                }
            )
            .await
            .unwrap()
            .id,
        f.run.id
    );
    stop.cancel();
    worker.await.unwrap();
    f.runtime.store.close().await;
}
