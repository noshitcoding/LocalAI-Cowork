#[path = "workflows/input_routes.rs"]
mod workflow_input_routes;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_domain::workflows::*;
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use cowork_workflow_contracts::{WorkflowRunKind, WorkflowRunRequest};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "workflow-product-test-token";
struct Fixture {
    app: axum::Router,
    runtime: Runtime,
    project: Project,
    order: Order,
    profile: ProviderProfile,
}
fn budgets() -> Budgets {
    Budgets {
        max_steps: 32,
        max_seconds: 60,
        max_output_tokens: 256,
        max_total_tokens: 131_072,
    }
}
fn definition(profile: &ProviderProfile) -> WorkflowDefinition {
    let rights = WorkflowRights {
        current_workspace: false,
        tools: ["create_artifact", "list_artifacts", "read_artifact"]
            .into_iter()
            .map(String::from)
            .collect(),
        folders: Default::default(),
    };
    WorkflowDefinition {
        documents: Default::default(),
        iterations: Default::default(),
        subflows: Default::default(),
        controls: Default::default(),
        gates: Default::default(),
        dataflow: None,
        format_version: 1,
        name: "Artifact workflow".into(),
        mode: WorkflowMode::Sequential,
        failure: WorkflowFailure::Stop,
        rights: rights.clone(),
        budgets: budgets(),
        roles: vec![WorkflowRole {
            agent_config: None,
            model_binding: None,
            output: None,
            id: "writer".into(),
            instructions: "Create the requested text result.".into(),
            provider_id: profile.id.clone(),
            provider_revision: profile.revision,
            rights: rights.clone(),
            budgets: budgets(),
            delegate_to: Default::default(),
        }],
        steps: vec![WorkflowStep {
            execution: None,
            zero_context: false,
            id: "create".into(),
            role_id: "writer".into(),
            goal: "Create an artifact.".into(),
            dependencies: vec![],
            rights,
            budgets: budgets(),
        }],
    }
}
async fn setup() -> Fixture {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Workflow API".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Create a result".into(),
        })
        .await
        .unwrap();
    let profile = ProviderProfile {
        generation: Default::default(),
        id: "api-test-model".into(),
        name: "API test model".into(),
        kind: ProviderKind::Test,
        endpoint: "".into(),
        model: "test".into(),
        revision: 1,
        context_tokens: 16_384,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    store.insert_provider(&profile).await.unwrap();
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile: profile.clone(),
            credential_env: None,
        }],
        true,
    )
    .unwrap();
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new(TOKEN.into()),
            native_token: None,
            origins: Arc::new(vec!["http://127.0.0.1:5175".into()]),
        },
        None,
    );
    Fixture {
        app,
        runtime,
        project,
        order,
        profile,
    }
}
async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
    let payload = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(request.body(payload).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn create(fixture: &Fixture) -> Value {
    let (status, body) = call(
        &fixture.app,
        "POST",
        &format!("/api/v2/projects/{}/workflows", fixture.project.id),
        Some(json!({"idempotency_key":"create", "definition":definition(&fixture.profile)})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}
fn start(fixture: &Fixture, library: &Value) -> WorkflowRunRequest {
    WorkflowRunRequest {
        input_source: None,
        inputs: None,
        agent_config: None,
        native_tools: vec![],
        folders: vec![],
        mcp_tools: vec![],
        environment: None,
        kind: WorkflowRunKind::Workflow,
        order_id: fixture.order.id.clone(),
        base_revision: fixture.order.revision,
        idempotency_key: "start".into(),
        prompt: "Create a result.".into(),
        workflow_id: library["id"].as_str().unwrap().into(),
        version_id: library["versions"][0]["id"].as_str().unwrap().into(),
        access_mode: AccessMode::ConfirmMutations,
        budgets: budgets(),
        expected_binding: None,
    }
}

#[tokio::test]
async fn workflow_routes_keep_token_and_origin_checks() {
    let fixture = setup().await;
    for (method, path) in [
        (
            "GET",
            format!("/api/v2/projects/{}/workflows", fixture.project.id),
        ),
        (
            "POST",
            format!("/api/v2/projects/{}/workflows", fixture.project.id),
        ),
        (
            "GET",
            format!("/api/v2/projects/{}/workflows/example", fixture.project.id),
        ),
        (
            "GET",
            format!(
                "/api/v2/projects/{}/workflows/example/versions/version",
                fixture.project.id
            ),
        ),
        (
            "POST",
            format!(
                "/api/v2/projects/{}/workflows/example/versions",
                fixture.project.id
            ),
        ),
        (
            "POST",
            format!(
                "/api/v2/projects/{}/workflows/example/revoke",
                fixture.project.id
            ),
        ),
        ("GET", "/api/v2/runs/example/workflow".into()),
    ] {
        let response = fixture
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
        let response = fixture
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&path)
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::ORIGIN, "https://hostile.invalid")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
    }
}

#[tokio::test]
async fn library_crud_keeps_immutable_versions_and_project_scope() {
    let fixture = setup().await;
    let library = create(&fixture).await;
    let base = format!(
        "/api/v2/projects/{}/workflows/{}",
        fixture.project.id,
        library["id"].as_str().unwrap()
    );
    let original = format!(
        "{base}/versions/{}",
        library["versions"][0]["id"].as_str().unwrap()
    );
    let (status, initial) = call(&fixture.app, "GET", &original, None).await;
    assert_eq!(status, StatusCode::OK);
    let mut updated = definition(&fixture.profile);
    updated.name = "Updated workflow".into();
    let publish = json!({"base_revision":1,"idempotency_key":"publish","definition":updated});
    let (status, published) = call(
        &fixture.app,
        "POST",
        &format!("{base}/versions"),
        Some(publish.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(published["revision"], 2);
    assert_eq!(
        call(
            &fixture.app,
            "POST",
            &format!("{base}/versions"),
            Some(publish)
        )
        .await
        .1,
        published
    );
    let (status, _) = call(&fixture.app, "POST", &format!("{base}/versions"), Some(json!({"base_revision":1,"idempotency_key":"stale","definition":definition(&fixture.profile)}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(call(&fixture.app, "GET", &original, None).await.1, initial);
    let (status, page) = call(
        &fixture.app,
        "GET",
        &format!("/api/v2/projects/{}/workflows?limit=1", fixture.project.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let foreign = fixture
        .runtime
        .store
        .create_project(CreateProject {
            name: "Other project".into(),
        })
        .await
        .unwrap();
    let (status, _) = call(
        &fixture.app,
        "GET",
        &format!(
            "/api/v2/projects/{}/workflows/{}",
            foreign.id,
            library["id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &fixture.app,
        "POST",
        &format!("{base}/revoke"),
        Some(json!({"base_revision":2,"idempotency_key":"revoke"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(&fixture.app, "GET", &base, None).await.1["revoked"],
        true
    );
    assert_eq!(call(&fixture.app, "GET", &original, None).await.1, initial);
}

#[tokio::test]
async fn workflow_selection_preflights_and_replays_without_browser_grants() {
    let fixture = setup().await;
    let library = create(&fixture).await;
    let mut request = start(&fixture, &library);
    let (status, flight) = call(
        &fixture.app,
        "POST",
        "/api/v2/preflight",
        Some(json!(request)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{flight}");
    assert!(flight.get("provider").is_none());
    assert_eq!(flight["roles"][0]["provider"]["id"], fixture.profile.id);
    request.expected_binding = Some(serde_json::from_value(flight["workflow"].clone()).unwrap());
    let request_json = json!(&request);
    let (status, error) = call(
        &fixture.app,
        "POST",
        "/api/v2/runs",
        Some(request_json.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["code"], "execution_plan_required");
    assert!(
        fixture
            .runtime
            .store
            .runs(&fixture.order.id)
            .await
            .unwrap()
            .is_empty()
    );
    let (status, prepared) = call(
        &fixture.app,
        "POST",
        "/api/v2/execution-plans",
        Some(request_json.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{prepared}");
    assert_eq!(
        prepared["plan"]["preflight"]["workflow"],
        flight["workflow"]
    );
    assert!(
        fixture
            .runtime
            .store
            .runs(&fixture.order.id)
            .await
            .unwrap()
            .is_empty()
    );
    let commit_path = format!(
        "/api/v2/orders/{}/execution-plans/{}/commit",
        fixture.order.id,
        prepared["plan"]["id"].as_str().unwrap()
    );
    let confirmation = json!({"sha256":prepared["sha256"]});
    let (status, run) = call(
        &fixture.app,
        "POST",
        &commit_path,
        Some(confirmation.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!(run["spec"]["workflow"], flight["workflow"]);
    assert!(run["spec"].get("provider").is_none_or(Value::is_null));
    assert!(run["spec"]["folders"].as_array().unwrap().is_empty());
    assert_eq!(
        call(&fixture.app, "POST", &commit_path, Some(confirmation)).await,
        (StatusCode::OK, run.clone())
    );
    assert_eq!(
        call(&fixture.app, "POST", "/api/v2/runs", Some(request_json)).await,
        (StatusCode::OK, run.clone())
    );
    let (status, view) = call(
        &fixture.app,
        "GET",
        &format!("/api/v2/runs/{}/workflow", run["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    for private in ["lease", "turns", "request_sha256", "seed", "checkpoint"] {
        assert!(
            !view.to_string().contains(&format!("\"{private}\"")),
            "private execution authority leaked: {private}"
        );
    }
    assert_eq!(
        fixture
            .runtime
            .store
            .runs(&fixture.order.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn forged_workflow_start_fields_are_rejected_with_structured_errors() {
    let fixture = setup().await;
    let library = create(&fixture).await;
    let request = json!(start(&fixture, &library));
    for (name, value) in [
        ("folders", json!([{"id":"forged","root":"C:/private"}])),
        (
            "mcp_tools",
            json!([{"server_id":"forged","tool_name":"execute"}]),
        ),
        ("max_parallel", json!(32)),
        ("spec", json!({"allowed_tools":["execute_program"]})),
        ("provider_id", json!("foreign-profile")),
    ] {
        let mut forged = request.clone();
        forged[name] = value;
        let (status, error) = call(&fixture.app, "POST", "/api/v2/runs", Some(forged)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {error}");
        assert_eq!(error["code"], "invalid_request");
        assert!(error["correlation_id"].is_string());
    }
    assert!(
        fixture
            .runtime
            .store
            .runs(&fixture.order.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn future_library_capabilities_cannot_be_started_through_public_api() {
    let fixture = setup().await;
    let mut unsupported = definition(&fixture.profile);
    // File reads are supported. Select an actually unknown capability at all
    // rights levels so narrowing cannot simply remove it before admission.
    unsupported
        .rights
        .tools
        .insert("unsupported_product_capability".into());
    unsupported.roles[0]
        .rights
        .tools
        .insert("unsupported_product_capability".into());
    unsupported.steps[0]
        .rights
        .tools
        .insert("unsupported_product_capability".into());
    let (status, library) = call(
        &fixture.app,
        "POST",
        &format!("/api/v2/projects/{}/workflows", fixture.project.id),
        Some(json!({"idempotency_key":"future","definition":unsupported})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &fixture.app,
        "POST",
        "/api/v2/execution-plans",
        Some(json!(start(&fixture, &library))),
    )
    .await;
    assert!(matches!(
        status,
        StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN
    ));
    assert!(
        fixture
            .runtime
            .store
            .runs(&fixture.order.id)
            .await
            .unwrap()
            .is_empty()
    );
}
