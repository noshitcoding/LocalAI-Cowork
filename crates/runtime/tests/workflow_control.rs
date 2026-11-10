use cowork_contracts::*;
use cowork_domain::workflows::*;
use cowork_runtime::Runtime;
use cowork_storage::Store;
use cowork_workflow_contracts::{CommitExecutionPlan, RunRequest, WorkflowRunRequest};
use serde_json::json;
use tokio_util::sync::CancellationToken;

async fn run_native(amount: u32, ambiguous: bool, restart: bool, numeric_equal: bool) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("native.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Native controls".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Amount routing".into(),
        })
        .await
        .unwrap();
    let mut definition: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-control.json"
    ))
    .unwrap();
    if ambiguous {
        definition.gates.clear();
    }
    if numeric_equal {
        definition.controls.get_mut("large").unwrap().operation =
            WorkflowControlOperation::Condition {
                predicate: WorkflowPredicate::Equal {
                    input: "amount".into(),
                    value: Box::new(json!(25.0)),
                },
            };
    }
    let library = store
        .create_workflow(&project.id, "native-controls", &definition)
        .await
        .unwrap();
    let request:WorkflowRunRequest=serde_json::from_value(json!({"kind":"workflow","order_id":order.id,"base_revision":order.revision,"idempotency_key":"native-run","prompt":"Run the declared native controls.","workflow_id":library.id,"version_id":library.versions[0].id,"access_mode":"read_only","budgets":definition.budgets,"inputs":{"amount":amount}})).unwrap();
    // An empty provider registry is intentional. Neither a probe nor a model
    // request can succeed by accidentally reaching a configured test provider.
    let mut runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_file_broker(
            cowork_broker::FileBroker::open(&temp.path().join("broker"), vec![]).unwrap(),
        );
    let prepared = runtime
        .prepare_execution_plan(&RunRequest::Workflow(request.clone()))
        .await
        .unwrap();
    let run = runtime
        .commit_execution_plan(
            &order.id,
            &prepared.plan.id,
            &CommitExecutionPlan {
                sha256: prepared.sha256,
            },
        )
        .await
        .unwrap();
    if restart {
        use cowork_storage::{WorkflowCommand, WorkflowCommandResult};
        let parent = runtime
            .store
            .claim_workflow_run(&project.id, &run.id)
            .await
            .unwrap();
        let lease = parent.lease.as_ref().unwrap().token.clone();
        let graph = runtime
            .store
            .stored_workflow(&project.id, &run.id)
            .await
            .unwrap();
        let WorkflowCommandResult::Claimed(claim) = runtime
            .store
            .workflow_command(
                &project.id,
                &run.id,
                &lease,
                graph.progress.revision(),
                WorkflowCommand::Claim {
                    node_id: "large".into(),
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        let graph = runtime
            .store
            .stored_workflow(&project.id, &run.id)
            .await
            .unwrap();
        runtime
            .store
            .workflow_command(
                &project.id,
                &run.id,
                &lease,
                graph.progress.revision(),
                WorkflowCommand::Control {
                    claim,
                    id: "before-restart".into(),
                },
            )
            .await
            .unwrap();
        let graph = runtime
            .store
            .stored_workflow(&project.id, &run.id)
            .await
            .unwrap();
        runtime
            .store
            .workflow_command(
                &project.id,
                &run.id,
                &lease,
                graph.progress.revision(),
                WorkflowCommand::Claim {
                    node_id: "review".into(),
                },
            )
            .await
            .unwrap();
        runtime
            .store
            .interrupt_workflow_run(&run.id, &lease, "fixture_restart")
            .await
            .unwrap();
        assert_eq!(
            runtime.store.resume(&run.id).await.unwrap().state,
            RunState::Queued
        );
        runtime.store.close().await;
        runtime = Runtime::new(Store::sqlite(&path).await.unwrap(), vec![], false)
            .unwrap()
            .with_file_broker(
                cowork_broker::FileBroker::open(&temp.path().join("broker"), vec![]).unwrap(),
            );
    }
    let result = runtime
        .run_native_workflow(&project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        result.state,
        if ambiguous {
            RunState::Failed
        } else {
            RunState::Completed
        },
        "{result:?}"
    );
    assert!(result.model_attempts.is_empty());
    let graph = runtime
        .store
        .stored_workflow(&project.id, &run.id)
        .await
        .unwrap();
    graph.validate(&result).unwrap();
    assert!(graph.agents.is_empty());
    assert_eq!(graph.progress.charged_tokens(), 0);
    assert!(
        graph
            .progress
            .attempts()
            .values()
            .all(|a| matches!(a.operation, WorkflowOperation::Control { .. })
                && a.reserved_tokens == 0)
    );
    if !ambiguous {
        let skipped = if (numeric_equal && amount == 25) || (!numeric_equal && amount > 100) {
            "automatic"
        } else {
            "review"
        };
        assert!(matches!(
            graph.progress.nodes()[skipped].state,
            WorkflowNodeState::Skipped { .. }
        ));
        let view = runtime
            .store
            .workflow_run_view(&project.id, &run.id)
            .await
            .unwrap();
        assert!(view.nodes.iter().any(|n| n.id == skipped
            && n.state == cowork_workflow_contracts::WorkflowNodeStatus::Skipped
            && n.role_id.is_none()));
    }
    let reopened = Store::sqlite(&path).await.unwrap();
    let saved = reopened
        .stored_workflow(&project.id, &run.id)
        .await
        .unwrap();
    saved.validate(&result).unwrap();
    assert_eq!(
        serde_json::to_value(&saved.progress).unwrap(),
        serde_json::to_value(&graph.progress).unwrap()
    );
    let mut forged = saved.clone();
    forged.binding.inputs.as_mut().unwrap()["amount"] = json!(999);
    let mut parent = result.clone();
    parent.spec.workflow = Some(forged.binding.clone());
    assert!(
        forged.validate(&parent).is_err(),
        "Stored native input is bound independently of the parent copy"
    );
    reopened.close().await;
}
#[tokio::test]
async fn native_controls_complete_with_no_configured_provider() {
    Box::pin(run_native(150, false, false, false)).await;
    Box::pin(run_native(25, false, false, false)).await;
}
#[tokio::test]
async fn ambiguous_merge_fails_without_a_model_or_tool_dispatch() {
    Box::pin(run_native(25, true, false, false)).await;
}

#[tokio::test]
async fn native_decision_and_unfinished_claim_survive_explicit_resume_and_database_reopen() {
    Box::pin(run_native(150, false, true, false)).await;
}

#[tokio::test]
async fn native_numeric_equality_routes_integer_input_and_float_constant_identically() {
    Box::pin(run_native(25, false, false, true)).await;
}
