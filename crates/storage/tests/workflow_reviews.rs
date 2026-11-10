//! Native review uses the real persisted human journal with no provider fixture.
use cowork_contracts::*;
use cowork_domain::{canonical_hash, workflows::*};
use cowork_storage::{Store, WorkflowCommand, WorkflowCommandResult, WorkflowToolExecution, id};
use cowork_workflow_contracts::*;
use serde_json::{Value, json};

fn definition() -> WorkflowDefinition {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-native-review.json"
    ))
    .unwrap()
}
fn correct() -> Value {
    json!({"invoice":"INV-REVIEW-42","issued":"2026-11-01","net_minor":10000,"tax_minor":1900,"gross_minor":11900})
}
struct Fixture {
    store: Store,
    run: RunRecord,
    lease: String,
    claim: WorkflowClaim,
    turn: String,
    input: WorkflowInputView,
}
async fn open(path: &std::path::Path) -> Fixture {
    let store = Store::sqlite(path).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Native human review".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Review without a model".into(),
        })
        .await
        .unwrap();
    let d = definition();
    let library = store
        .create_workflow(&project.id, "create-review", &d)
        .await
        .unwrap();
    let initial =
        json!({"invoice":null,"issued":null,"net_minor":null,"tax_minor":null,"gross_minor":null});
    let spec:RunSpec=serde_json::from_value(json!({"project_id":project.id,"order_id":order.id,"prompt":"Check invoice values before booking.","provider":null,"access_mode":"confirm_mutations","executor":"local-v2","policy_revision":1,"allowed_tools":["create_artifact","list_artifacts","read_artifact","request_input"],"budgets":d.budgets,"folders":[],"mcp_tools":[],"workflow":null,"environment":null,"file_action":null,"output":null,"model_binding":null,"agent_config":null})).unwrap();
    let request:WorkflowRunRequest=serde_json::from_value(json!({"kind":"workflow","order_id":order.id,"base_revision":order.revision,"idempotency_key":"start-native-review","workflow_id":library.id,"version_id":library.versions[0].id,"prompt":spec.prompt,"access_mode":"confirm_mutations","budgets":d.budgets,"inputs":initial,"expected_binding":null,"native_tools":[],"folders":[],"mcp_tools":[],"environment":null,"agent_config":null})).unwrap();
    let plan = store
        .product_preflight_compilation(&request, &spec)
        .await
        .unwrap();
    let binding = WorkflowBinding {
        inputs: request.inputs.clone(),
        workflow_id: request.workflow_id.clone(),
        version_id: request.version_id.clone(),
        definition_sha256: plan.source_sha256.clone(),
        plan_sha256: plan.sha256.clone(),
    };
    let run = store
        .create_product_workflow_run(&request, spec, &binding)
        .await
        .unwrap();
    let parent = store
        .claim_workflow_run(&project.id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let workflow = store.stored_workflow(&project.id, &run.id).await.unwrap();
    let WorkflowCommandResult::Claimed(claim) = store
        .workflow_command(
            &project.id,
            &run.id,
            &lease,
            workflow.progress.revision(),
            WorkflowCommand::Claim {
                node_id: "review".into(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("claim")
    };
    let spec = store
        .workflow_node_spec(&project.id, &run.id, "review")
        .await
        .unwrap();
    assert!(
        spec.provider.is_none()
            && spec.model_binding.is_none()
            && spec.folders.is_empty()
            && spec.environment.is_none()
    );
    let turn_id = id();
    let turn = store
        .prepare_workflow_step(
            &project.id,
            &run.id,
            &lease,
            &claim,
            &turn_id,
            cowork_domain::model_tool_catalog(&spec),
        )
        .await
        .unwrap();
    assert!(turn.deterministic);
    let WorkflowToolExecution::AwaitingInput(input) = store
        .execute_workflow_artifact_tool(&project.id, &run.id, &lease, &turn.id, "native-review")
        .await
        .unwrap()
    else {
        panic!("native human wait")
    };
    assert_eq!(input.role_id, None);
    assert!(input.review.is_some() && input.actionable);
    Fixture {
        store,
        run,
        lease,
        claim,
        turn: turn_id,
        input: *input,
    }
}
async fn save(f: &Fixture, base: i64, key: &str, value: &Value) -> WorkflowInputDraft {
    let WorkflowInputDraftSave::Saved { draft, .. } = f
        .store
        .save_workflow_input_draft(
            &f.run.id,
            &f.input.request_id,
            &SaveWorkflowInputDraft {
                fingerprint: f.input.fingerprint.clone(),
                base_revision: base,
                idempotency_key: key.into(),
                answer: value.to_string(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("saved draft")
    };
    draft
}
#[tokio::test]
async fn native_review_requires_semantic_correction_and_original_confirmed_intent_exactly_once() {
    let temp = tempfile::tempdir().unwrap();
    let f = open(&temp.path().join("review.sqlite3")).await;
    let lookup = f
        .store
        .workflow_input_draft(&f.run.id, &f.input.request_id)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&lookup.draft.answer).unwrap(),
        f.input.review.as_ref().unwrap().values
    );
    assert_eq!(lookup.draft.revision, 0);
    assert_eq!(lookup.draft.snapshot_id, None);
    let initial = f.input.review.as_ref().unwrap().values.clone();
    let bad = save(&f, 0, "save-incomplete", &initial).await;
    assert!(
        f.store
            .prepare_workflow_input_answer(
                &f.run.id,
                &f.input.request_id,
                &PrepareWorkflowInputAnswer {
                    fingerprint: f.input.fingerprint.clone(),
                    base_draft_revision: bad.revision,
                    idempotency_key: "reject-semantic-error".into()
                }
            )
            .await
            .is_err()
    );
    assert!(
        f.store
            .workflow_input_draft(&f.run.id, &f.input.request_id)
            .await
            .unwrap()
            .intents
            .is_empty()
    );
    let good = save(&f, bad.revision, "save-corrected", &correct()).await;
    let intent = f
        .store
        .prepare_workflow_input_answer(
            &f.run.id,
            &f.input.request_id,
            &PrepareWorkflowInputAnswer {
                fingerprint: f.input.fingerprint.clone(),
                base_draft_revision: good.revision,
                idempotency_key: "confirm-corrected".into(),
            },
        )
        .await
        .unwrap();
    let once = f
        .store
        .commit_workflow_input_answer_intent(&f.run.id, &intent.id)
        .await
        .unwrap();
    let twice = f
        .store
        .commit_workflow_input_answer_intent(&f.run.id, &intent.id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&once), canonical_hash(&twice));
    assert_eq!(once.state, WorkflowInputAnswerIntentState::Accepted);
    let saved = f
        .store
        .workflow_model_turn(&f.run.spec.project_id, &f.run.id, &f.turn)
        .await
        .unwrap()
        .unwrap();
    let output = cowork_storage::deterministic_step_output(&saved).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&output).unwrap(), correct());
    let workflow = f
        .store
        .stored_workflow(&f.run.spec.project_id, &f.run.id)
        .await
        .unwrap();
    assert_eq!(workflow.progress.attempts().len(), 2);
    assert_eq!(workflow.progress.charged_tokens(), 0);
    f.store
        .workflow_command(
            &f.run.spec.project_id,
            &f.run.id,
            &f.lease,
            workflow.progress.revision(),
            WorkflowCommand::Complete {
                claim: f.claim.clone(),
                output,
            },
        )
        .await
        .unwrap();
    let final_state = f
        .store
        .stored_workflow(&f.run.spec.project_id, &f.run.id)
        .await
        .unwrap();
    assert_eq!(final_state.progress.state(), WorkflowState::Completed);
    assert!(final_state.plan.profiles.is_empty());
}

#[tokio::test]
async fn waiting_review_and_incomplete_draft_survive_database_reopen_without_new_request() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("review.sqlite3");
    let mut f = open(&path).await;
    let draft = save(
        &f,
        0,
        "preserve-incomplete",
        &f.input.review.as_ref().unwrap().values,
    )
    .await;
    let original = f
        .store
        .stored_workflow(&f.run.spec.project_id, &f.run.id)
        .await
        .unwrap();
    let binding = canonical_hash(&original.inputs);
    f.store.close().await;
    f.store = Store::sqlite(&path).await.unwrap();
    let reopened = f
        .store
        .stored_workflow(&f.run.spec.project_id, &f.run.id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&reopened.inputs), binding);
    assert_eq!(reopened.inputs.len(), 1);
    let lookup = f
        .store
        .workflow_input_draft(&f.run.id, &f.input.request_id)
        .await
        .unwrap();
    assert_eq!(canonical_hash(&lookup.draft), canonical_hash(&draft));
    assert!(lookup.intents.is_empty());
    assert_eq!(reopened.progress.charged_tokens(), 0);
    assert_eq!(reopened.agents.len(), 1);
    assert_eq!(reopened.agents["review"].turns.len(), 1);
}

#[tokio::test]
async fn stored_review_schema_prefill_and_role_are_bound_to_the_original_definition() {
    let temp = tempfile::tempdir().unwrap();
    let f = open(&temp.path().join("review.sqlite3")).await;
    let original = f
        .store
        .stored_workflow(&f.run.spec.project_id, &f.run.id)
        .await
        .unwrap();
    let run = f.store.stored_run(&f.run.id).await.unwrap();
    original.validate(&run.record).unwrap();
    let mut wrong = original.clone();
    wrong
        .inputs
        .get_mut(&f.input.request_id)
        .unwrap()
        .review
        .as_mut()
        .unwrap()
        .form
        .fields
        .get_mut("invoice")
        .unwrap()
        .label = "Different meaning".into();
    assert!(wrong.validate(&run.record).is_err());
    let mut wrong = original.clone();
    wrong
        .inputs
        .get_mut(&f.input.request_id)
        .unwrap()
        .review
        .as_mut()
        .unwrap()
        .values["gross_minor"] = json!(99999);
    assert!(wrong.validate(&run.record).is_err());
    let mut wrong = original.clone();
    wrong.inputs.get_mut(&f.input.request_id).unwrap().role_id =
        Some("fabricated-model-role".into());
    assert!(wrong.validate(&run.record).is_err());
}
