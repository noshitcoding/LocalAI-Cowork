//! Functional actual host OCR and human review; no parser-isolation claim.
use super::*;
use cowork_domain::canonical_hash;
use cowork_workflow_contracts::*;
use serde_json::Value;
mod extraction;

fn corrected() -> Value {
    json!({"invoice":"INV-SOURCE-42","issued":"2026-11-01","net_minor":10000,"tax_minor":1900,"gross_minor":11900})
}
async fn fixture() -> Fixture {
    let mut f = Box::pin(Fixture::new(false)).await;
    f.ocr_source(true, 0, 100).await;
    let definition: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/workflow-native-review-sources.json"
    ))
    .unwrap();
    let entry = f
        .runtime
        .store
        .create_workflow(&f.project.id, "native-ocr-human-source", &definition)
        .await
        .unwrap();
    f.workflow = entry.id;
    f.version = entry.versions[0].id.clone();
    f
}
fn worker(f: &Fixture) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let stop = CancellationToken::new();
    let task = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    (stop, task)
}
async fn question(f: &Fixture, run: &RunRecord) -> WorkflowInputView {
    let until = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let view = f
            .runtime
            .store
            .workflow_run_view(&f.project.id, &run.id)
            .await
            .unwrap();
        if let Some(input) = view.inputs.into_iter().find(|input| input.actionable) {
            return input;
        }
        assert!(
            matches!(
                view.run.state,
                RunState::Queued | RunState::Running | RunState::WaitingForSnapshot
            ),
            "{:?}",
            view.run
        );
        assert!(
            tokio::time::Instant::now() < until,
            "OCR review source did not become actionable"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
async fn answer(
    f: &Fixture,
    run: &RunRecord,
    input: &WorkflowInputView,
) -> WorkflowInputAnswerIntent {
    let head = f
        .runtime
        .store
        .workflow_input_draft(&run.id, &input.request_id)
        .await
        .unwrap();
    let WorkflowInputDraftSave::Saved { draft, .. } = f
        .runtime
        .store
        .save_workflow_input_draft(
            &run.id,
            &input.request_id,
            &SaveWorkflowInputDraft {
                fingerprint: input.fingerprint.clone(),
                base_revision: head.draft.revision,
                idempotency_key: "source-review-corrected".into(),
                answer: corrected().to_string(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("draft save")
    };
    f.runtime
        .store
        .prepare_workflow_input_answer(
            &run.id,
            &input.request_id,
            &PrepareWorkflowInputAnswer {
                fingerprint: input.fingerprint.clone(),
                base_draft_revision: draft.revision,
                idempotency_key: "source-review-original-intent".into(),
            },
        )
        .await
        .unwrap()
}
async fn assert_source(f: &Fixture, run: &RunRecord, input: &WorkflowInputView) {
    assert!(input.role_id.is_none());
    let review = input.review.as_ref().unwrap();
    assert_eq!(review.evidence.len(), 1);
    assert_eq!(review.form.source_steps, ["read-source"]);
    assert_eq!(review.evidence[0].step_id, "read-source");
    assert_eq!(review.evidence[0].kind, WorkflowReviewEvidenceKind::Ocr);
    let evidence = &review.evidence[0].output;
    assert_eq!(evidence["source"]["sha256"], f.source_sha);
    assert!(!evidence["words"].as_array().unwrap().is_empty());
    assert!(evidence["text"].as_str().unwrap().contains("119"));
    assert!(evidence["pages"][0]["preview_sha256"].as_str().is_some());
    assert!(
        review
            .values
            .as_object()
            .unwrap()
            .values()
            .all(Value::is_null)
    );
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let WorkflowNodeState::Completed { output, .. } = &stored.progress.nodes()["read-source"].state
    else {
        panic!("completed OCR source")
    };
    assert_eq!(serde_json::from_str::<Value>(output).unwrap(), *evidence);
    let record = f.runtime.store.stored_run(&run.id).await.unwrap();
    assert!(record.record.model_attempts.is_empty());
    assert!(stored.plan.roles.is_empty() && stored.plan.profiles.is_empty());
    assert!(stored.plan.nodes["review"].rights.folders.is_empty());
    let mut forged = stored.clone();
    forged
        .inputs
        .get_mut(&input.request_id)
        .unwrap()
        .review
        .as_mut()
        .unwrap()
        .evidence[0]
        .output["text"] = json!("Forged later text");
    assert!(forged.validate(&record.record).is_err());
    let mut forged = stored.clone();
    forged
        .inputs
        .get_mut(&input.request_id)
        .unwrap()
        .review
        .as_mut()
        .unwrap()
        .evidence[0]
        .step_id = "unselected-step".into();
    assert!(forged.validate(&record.record).is_err());
}
async fn completed(f: &Fixture, run: &RunRecord) {
    let detail = f.completed(&run.id).await;
    assert_eq!(detail.run.state, RunState::Completed);
    assert!(detail.file_operations.is_empty() && detail.approvals.is_empty());
    f.assert_native(&run.id, 2).await;
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(graph.progress.attempts().len(), 4);
    let WorkflowNodeState::Completed { output, .. } = &graph.progress.nodes()["review"].state
    else {
        panic!("review completed")
    };
    assert_eq!(serde_json::from_str::<Value>(output).unwrap(), corrected());
}

#[tokio::test]
#[ignore = "requires explicitly configured actual OCR Python and language pack"]
async fn actual_ocr_source_is_immutable_review_context_and_cannot_be_replaced_by_later_original_edits()
 {
    let f = Box::pin(fixture()).await;
    let run = f.commit(AccessMode::ReadOnly).await;
    let changed = b"Changed original after explicit frozen-source plan";
    std::fs::write(f.original().join("invoice.pdf"), changed).unwrap();
    let (stop, task) = worker(&f);
    let input = question(&f, &run).await;
    assert_source(&f, &run, &input).await;
    let intent = answer(&f, &run, &input).await;
    let once = f
        .runtime
        .store
        .commit_workflow_input_answer_intent(&run.id, &intent.id)
        .await
        .unwrap();
    completed(&f, &run).await;
    let twice = f
        .runtime
        .store
        .commit_workflow_input_answer_intent(&run.id, &intent.id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&once), canonical_hash(&twice));
    assert_eq!(
        std::fs::read(f.original().join("invoice.pdf")).unwrap(),
        changed
    );
    stop.cancel();
    task.await.unwrap();
    f.runtime.store.close().await;
}

#[tokio::test]
#[ignore = "requires explicitly configured actual OCR Python and language pack"]
async fn original_ocr_evidence_and_answer_intent_survive_waiting_restart_without_repeating_the_parser()
 {
    let mut f = Box::pin(fixture()).await;
    let run = f.commit(AccessMode::ReadOnly).await;
    let (stop, task) = worker(&f);
    let input = question(&f, &run).await;
    assert_source(&f, &run, &input).await;
    let intent = answer(&f, &run, &input).await;
    let before = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let plan = canonical_hash(&before.plan);
    let attempts = before.progress.attempts().clone();
    stop.cancel();
    task.await.unwrap();
    f.runtime.store.close().await;
    f.runtime = Runtime::new(
        Store::sqlite(&f.temp.path().join("native-documents.sqlite3"))
            .await
            .unwrap(),
        vec![],
        false,
    )
    .unwrap()
    .with_file_broker(
        cowork_broker::FileBroker::open(&f.temp.path().join("broker"), vec![]).unwrap(),
    )
    .with_toolbox(f.toolbox.clone());
    let view = f
        .runtime
        .store
        .workflow_run_view(&f.project.id, &run.id)
        .await
        .unwrap();
    let preview = view.input_resume_preview;
    assert!(preview.available, "{:?}", preview.blocked_reason);
    let request = ResumeWorkflowInput {
        run_id: run.id.clone(),
        base_run_revision: preview.base_run_revision,
        base_workflow_revision: preview.base_workflow_revision,
        expected_checkpoint_sha256: preview.expected_checkpoint_sha256,
        expected_cursor_sha256: preview.expected_cursor_sha256.unwrap(),
        idempotency_key: "resume-original-source-review".into(),
    };
    let op = f
        .runtime
        .store
        .prepare_workflow_operation(
            &f.project.id,
            &PrepareWorkflowOperation {
                intent_id: "resume-original-source-review".into(),
                request: WorkflowOperationRequest::ResumeInput { request },
                draft_snapshot: None,
            },
        )
        .await
        .unwrap();
    f.runtime
        .commit_workflow_operation(&f.project.id, &op.id)
        .await
        .unwrap();
    let (stop, task) = worker(&f);
    let restored = question(&f, &run).await;
    assert_eq!(restored.review, input.review);
    assert_eq!(restored.request_id, input.request_id);
    assert_eq!(restored.fingerprint, input.fingerprint);
    assert_eq!(restored.expires_at, input.expires_at);
    assert_source(&f, &run, &restored).await;
    assert_eq!(
        canonical_hash(
            &f.runtime
                .store
                .workflow_input_answer_intent(&run.id, &intent.id)
                .await
                .unwrap()
        ),
        canonical_hash(&intent)
    );
    let after = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(
        canonical_hash(after.progress.attempts()),
        canonical_hash(&attempts)
    );
    f.runtime
        .store
        .commit_workflow_input_answer_intent(&run.id, &intent.id)
        .await
        .unwrap();
    completed(&f, &run).await;
    assert_eq!(
        canonical_hash(
            &f.runtime
                .store
                .stored_workflow(&f.project.id, &run.id)
                .await
                .unwrap()
                .plan
        ),
        plan
    );
    stop.cancel();
    task.await.unwrap();
    f.runtime.store.close().await;
}
