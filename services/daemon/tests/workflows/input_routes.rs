use super::*;
use cowork_storage::{
    WorkflowCommand, WorkflowCommandResult, WorkflowModelAdmission, WorkflowModelResponse,
    WorkflowToolExecution, id,
};
async fn input_fixture() -> (
    Fixture,
    RunRecord,
    cowork_workflow_contracts::WorkflowInputView,
) {
    let f = setup().await;
    let mut def = definition(&f.profile);
    def.rights.tools.insert("request_input".into());
    for role in &mut def.roles {
        role.rights.tools.insert("request_input".into());
    }
    for step in &mut def.steps {
        step.rights.tools.insert("request_input".into());
    }
    let library = f
        .runtime
        .store
        .create_workflow(&f.project.id, "input-api-library", &def)
        .await
        .unwrap();
    let request = start(&f, &serde_json::to_value(&library).unwrap());
    let run = f
        .runtime
        .create_product_workflow_run(&request)
        .await
        .unwrap();
    let parent = f
        .runtime
        .store
        .claim_workflow_run(&f.project.id, &run.id)
        .await
        .unwrap();
    let lease = parent.lease.unwrap().token;
    let before = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let WorkflowCommandResult::Claimed(claim) = f
        .runtime
        .store
        .workflow_command(
            &f.project.id,
            &run.id,
            &lease,
            before.progress.revision(),
            WorkflowCommand::Claim {
                node_id: "create".into(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("node claim");
    };
    let catalog = json!([{"type":"function","function":{"name":"request_input","parameters":{"type":"object"}}}]);
    let (revision, context) = f
        .runtime
        .store
        .workflow_model_context(&f.project.id, &run.id, &claim, catalog)
        .await
        .unwrap();
    let turn = id();
    assert!(matches!(
        f.runtime
            .store
            .begin_workflow_model(
                &f.project.id,
                &run.id,
                &lease,
                revision,
                &claim,
                &turn,
                &context
            )
            .await
            .unwrap(),
        WorkflowModelAdmission::Dispatch(_)
    ));
    f.runtime
        .store
        .finish_workflow_model(
            &f.project.id,
            &run.id,
            &lease,
            &claim,
            &turn,
            WorkflowModelResponse {
                provider_continuation: None,
                received_cost_metadata: None,
                content: "confirmed request".into(),
                tools: vec![ToolCall {
                    id: "input".into(),
                    name: "request_input".into(),
                    arguments: json!({"question":"Which exact answer?"}),
                }],
                usage: Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(2),
                    ..Default::default()
                },
                rejection_code: None,
            },
        )
        .await
        .unwrap();
    let WorkflowToolExecution::AwaitingInput(q) = f
        .runtime
        .store
        .execute_workflow_artifact_tool(&f.project.id, &run.id, &lease, &turn, "input")
        .await
        .unwrap()
    else {
        panic!("human wait");
    };
    (f, run, *q)
}
#[tokio::test]
async fn exact_input_draft_cas_and_bodyless_frozen_commit_use_real_api() {
    let (f, run, q) = input_fixture().await;
    let path = format!(
        "/api/v2/runs/{}/workflow/inputs/{}/draft",
        run.id, q.request_id
    );
    let exact = "  ö😀\r\n\"quote\"\\path\n  ";
    let save = json!({"fingerprint":q.fingerprint,"base_revision":0,"idempotency_key":"save","answer":exact});
    let (status, initial) = call(&f.app, "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(initial["draft"]["revision"], 0);
    let (status, saved) = call(&f.app, "PUT", &path, Some(save.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["snapshot"]["answer"], exact);
    let (status, replay) = call(&f.app, "PUT", &path, Some(save)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, saved);
    let (status,conflict)=call(&f.app,"PUT",&path,Some(json!({"fingerprint":q.fingerprint,"base_revision":0,"idempotency_key":"stale-save","answer":"retained conflict"}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["outcome"], "conflict");
    assert_eq!(conflict["snapshot"]["answer"], "retained conflict");
    assert_eq!(conflict["draft"]["answer"], exact);
    let snapshot = conflict["snapshot"]["id"].as_str().unwrap();
    let (status, read) = call(&f.app, "GET", &format!("{path}/snapshots/{snapshot}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read, conflict["snapshot"]);
    let prepare = format!(
        "/api/v2/runs/{}/workflow/inputs/{}/answer-intents",
        run.id, q.request_id
    );
    let (status, intent) = call(
        &f.app,
        "POST",
        &prepare,
        Some(
            json!({"fingerprint":q.fingerprint,"base_draft_revision":1,"idempotency_key":"answer"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{intent}");
    assert_eq!(intent["submission"]["input"]["answer"], exact);
    let id = intent["id"].as_str().unwrap();
    let commit = format!(
        "/api/v2/runs/{}/workflow/input-answer-intents/{id}/commit",
        run.id
    );
    let (status, error) = call(&f.app, "POST", &commit, Some(json!({"answer":"forged"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_request");
    let (status, accepted) = call(&f.app, "POST", &commit, None).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["state"], "accepted");
    assert_eq!(accepted["submission"]["input"]["answer"], exact);
    let (status, replayed) = call(&f.app, "POST", &commit, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed, accepted);
    let (status, legacy) = call(
        &f.app,
        "POST",
        &format!("/api/v2/runs/{}/input", run.id),
        Some(json!({"request_id":q.request_id,"answer":"legacy bypass"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(legacy["code"], "forbidden");
}
#[tokio::test]
async fn input_api_rejects_supplied_authority_and_incomplete_submission() {
    let (f, run, q) = input_fixture().await;
    let draft = format!(
        "/api/v2/runs/{}/workflow/inputs/{}/draft",
        run.id, q.request_id
    );
    let prepare = format!(
        "/api/v2/runs/{}/workflow/inputs/{}/answer-intents",
        run.id, q.request_id
    );
    for field in [
        "owner",
        "project_id",
        "run_id",
        "request_id",
        "grant",
        "receipt",
    ] {
        let mut body = json!({"fingerprint":q.fingerprint,"base_revision":0,"idempotency_key":"save","answer":"safe"});
        body[field] = json!("forged");
        assert_eq!(
            call(&f.app, "PUT", &draft, Some(body)).await.0,
            StatusCode::BAD_REQUEST
        );
        let mut body = json!({"fingerprint":q.fingerprint,"base_draft_revision":1,"idempotency_key":"prepare"});
        body[field] = json!("forged");
        assert_eq!(
            call(&f.app, "POST", &prepare, Some(body)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let (status,_)=call(&f.app,"PUT",&draft,Some(json!({"fingerprint":q.fingerprint,"base_revision":0,"idempotency_key":"whitespace","answer":"  \r\n  "}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(call(&f.app,"POST",&prepare,Some(json!({"fingerprint":q.fingerprint,"base_draft_revision":1,"idempotency_key":"whitespace-answer"}))).await.0,StatusCode::BAD_REQUEST);
    for text in ["a\0b".into(), "😀".repeat(8001), "x".repeat(1_200_000)] {
        assert_eq!(call(&f.app,"PUT",&draft,Some(json!({"fingerprint":q.fingerprint,"base_revision":1,"idempotency_key":"invalid-text","answer":text}))).await.0,StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        call(
            &f.app,
            "GET",
            "/api/v2/runs/missing/workflow/input-answer-intents/foreign",
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/v2/runs/{}/workflow/inputs/foreign/draft", run.id),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!(
                "/api/v2/runs/{}/workflow/inputs/{}/draft",
                "x".repeat(129),
                q.request_id
            ),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
#[tokio::test]
async fn all_new_input_routes_share_existing_auth_and_origin_guards() {
    let f = setup().await;
    for (method, path) in [
        ("GET", "/api/v2/runs/r/workflow/inputs/q/draft"),
        ("PUT", "/api/v2/runs/r/workflow/inputs/q/draft"),
        ("GET", "/api/v2/runs/r/workflow/inputs/q/draft/snapshots/s"),
        ("POST", "/api/v2/runs/r/workflow/inputs/q/answer-intents"),
        ("GET", "/api/v2/runs/r/workflow/input-answer-intents/i"),
        (
            "POST",
            "/api/v2/runs/r/workflow/input-answer-intents/i/commit",
        ),
    ] {
        let unauthorized = f
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let foreign = f
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::ORIGIN, "https://foreign.invalid")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    }
}
