use super::*;
use cowork_storage::workflow_toolbox::NativeWorkflowAdmission;
#[tokio::test]
async fn deterministic_pipeline_preparation_has_no_model_usage_and_zero_context_omits_earlier_outputs()
 {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let mut f = setup(&store).await;
    f.definition.mode = WorkflowMode::Sequential;
    f.definition.rights.tools = BTreeSet::from(["browser".into()]);
    f.definition.budgets.max_steps = 16;
    for role in &mut f.definition.roles {
        role.rights = f.definition.rights.clone();
        role.budgets.max_steps = 16;
    }
    f.definition.steps.truncate(2);
    for step in &mut f.definition.steps {
        step.rights = f.definition.rights.clone();
        step.budgets.max_steps = 16;
    }
    f.definition.steps[1].execution = Some(WorkflowStepExecution::WebFetch {
        url: "https://example.invalid/evidence".into(),
    });
    f.definition.steps[1].zero_context = true;
    let entry = store
        .create_workflow(&f.spec.project_id, "pipeline-new", &f.definition)
        .await
        .unwrap();
    f.request.workflow_id = entry.id;
    f.request.version_id = entry.versions[0].id.clone();
    f.spec.allowed_tools = vec!["browser".into()];
    f.spec.executor = "owner-native-toolbox-v1".into();
    f.spec.access_mode = AccessMode::ConfirmAll;
    f.spec.budgets.max_steps = 16;
    let run = store
        .create_workflow_run(&f.request, f.spec.clone())
        .await
        .unwrap();
    let claimed = store
        .claim_workflow_run(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    let lease = claimed.lease.unwrap().token;
    let first = claim_node(&store, &f, &run, &lease, "a").await;
    let workflow = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    store
        .workflow_command(
            &f.spec.project_id,
            &run.id,
            &lease,
            workflow.progress.revision(),
            WorkflowCommand::Complete {
                claim: first,
                output: "PRIOR_STEP_MUST_NOT_LEAK".into(),
            },
        )
        .await
        .unwrap();
    let claim = claim_node(&store, &f, &run, &lease, "b").await;
    let spec = store
        .workflow_node_spec(&f.spec.project_id, &run.id, "b")
        .await
        .unwrap();
    let turn = store
        .prepare_workflow_step(
            &f.spec.project_id,
            &run.id,
            &lease,
            &claim,
            "deterministic-preparation",
            cowork_domain::model_tool_catalog(&spec),
        )
        .await
        .unwrap();
    assert!(turn.deterministic);
    assert!(
        !serde_json::to_string(&turn.context.messages)
            .unwrap()
            .contains("PRIOR_STEP_MUST_NOT_LEAK")
    );
    assert!(
        serde_json::to_string(&turn.context.messages)
            .unwrap()
            .contains(&f.spec.prompt)
    );
    assert_eq!(
        turn.response.as_ref().unwrap().tools[0].arguments["operation"],
        "fetch_text"
    );
    let parent = store.stored_run(&run.id).await.unwrap();
    assert!(parent.record.model_attempts.is_empty());
    let graph = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    assert_eq!(graph.progress.charged_tokens(), 0);
    assert!(matches!(
        graph.progress.attempts()[&turn.id].operation,
        WorkflowOperation::Preparation { .. }
    ));
    let NativeWorkflowAdmission::AwaitingApproval(approval) = store
        .admit_workflow_toolbox(
            &f.spec.project_id,
            &run.id,
            &lease,
            &turn.id,
            "pipeline-browser",
        )
        .await
        .unwrap()
    else {
        panic!("Exact native approval is required")
    };
    store
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
    assert!(matches!(
        store
            .admit_workflow_toolbox(
                &f.spec.project_id,
                &run.id,
                &lease,
                &turn.id,
                "pipeline-browser"
            )
            .await
            .unwrap(),
        NativeWorkflowAdmission::Dispatch { .. }
    ));
    assert!(
        store
            .admit_workflow_toolbox(
                &f.spec.project_id,
                &run.id,
                &lease,
                &turn.id,
                "pipeline-browser"
            )
            .await
            .is_err(),
        "An admitted web request is never granted a second dispatch"
    );
    store.finish_workflow_toolbox(&f.spec.project_id,&run.id,&lease,&turn.id,"pipeline-browser",json!({"state":"completed","outcome":{"exit_code":0,"termination":"exited","stdout":json!({"text":"Exact retained web result"}).to_string()}})).await.unwrap();
    let saved = store
        .workflow_model_turn(&f.spec.project_id, &run.id, &turn.id)
        .await
        .unwrap()
        .unwrap();
    let output = cowork_storage::deterministic_step_output(&saved).unwrap();
    let graph = store
        .stored_workflow(&f.spec.project_id, &run.id)
        .await
        .unwrap();
    store
        .workflow_command(
            &f.spec.project_id,
            &run.id,
            &lease,
            graph.progress.revision(),
            WorkflowCommand::Complete {
                claim,
                output: output.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(output, "Exact retained web result");
    assert!(
        store
            .stored_run(&run.id)
            .await
            .unwrap()
            .record
            .model_attempts
            .is_empty()
    );
}
