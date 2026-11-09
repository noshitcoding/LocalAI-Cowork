use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use cowork_workflow_contracts::*;
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

fn collection(fixture: &Fixture, name: &str) -> String {
    format!("/api/v2/projects/{}/{name}", fixture.project.id)
}
fn draft_body() -> Value {
    json!({"id":"editor","base_revision":0,"idempotency_key":"draft-save", "content":{"editor_format":"workflow-v1","data":{"incomplete":"role"},"archived":false}})
}
async fn prepare_create(fixture: &Fixture) -> Value {
    let (status, operation) = call(&fixture.app,"POST",&collection(fixture,"workflow-operations"),Some(json!({
        "intent_id":"create-intent","request":{"kind":"create","request":{"idempotency_key":"journal-create","definition":definition(&fixture.profile)}}
    }))).await;
    assert_eq!(status, StatusCode::OK, "{operation}");
    operation
}
#[tokio::test]
async fn journal_routes_require_existing_owner_token_and_origin() {
    let fixture = setup().await;
    let root = format!("/api/v2/projects/{}", fixture.project.id);
    for (method, suffix) in [
        ("GET", "workflow-drafts"),
        ("PUT", "workflow-drafts"),
        ("GET", "workflow-drafts/editor"),
        ("GET", "workflow-drafts/editor/snapshots"),
        ("GET", "workflow-drafts/editor/snapshots/one"),
        ("GET", "workflow-operations"),
        ("POST", "workflow-operations"),
        ("GET", "workflow-operations/one"),
        ("POST", "workflow-operations/one/commit"),
        ("POST", "workflow-operations/one/discard"),
        ("POST", "workflow-operations/one/acknowledge"),
    ] {
        for hostile in [false, true] {
            let mut request = Request::builder()
                .method(method)
                .uri(format!("{root}/{suffix}"));
            if hostile {
                request = request
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::ORIGIN, "https://hostile.invalid");
            }
            let response = fixture
                .app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if hostile {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::UNAUTHORIZED
                },
                "{method} {suffix}"
            );
        }
    }
}
#[tokio::test]
async fn journal_draft_conflicts_are_saved_typed_receipts_with_both_versions() {
    let fixture = setup().await;
    let path = collection(&fixture, "workflow-drafts");
    let original = draft_body();
    let (status, saved) = call(&fixture.app, "PUT", &path, Some(original.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["outcome"], "saved");
    assert_eq!(
        call(&fixture.app, "PUT", &path, Some(original)).await.1,
        saved
    );
    let mut current = draft_body();
    current["base_revision"] = json!(1);
    current["idempotency_key"] = json!("current-save");
    current["content"]["data"] = json!({"partial":"new"});
    let (status, updated) = call(&fixture.app, "PUT", &path, Some(current)).await;
    assert_eq!(status, StatusCode::OK);
    let mut stale = draft_body();
    stale["idempotency_key"] = json!("conflicting-save");
    stale["content"]["data"] = json!({"partial":"other"});
    let (status, conflict) = call(&fixture.app, "PUT", &path, Some(stale.clone())).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["outcome"], "conflict");
    assert_eq!(conflict["draft"], updated["draft"]);
    assert_eq!(conflict["snapshot"]["conflict"], true);
    assert_eq!(
        conflict["snapshot"]["content"]["data"],
        stale["content"]["data"]
    );
    assert!(conflict.get("correlation_id").is_none());
    assert_eq!(
        call(&fixture.app, "PUT", &path, Some(stale)).await,
        (StatusCode::CONFLICT, conflict.clone())
    );
    let (_, snapshots) = call(
        &fixture.app,
        "GET",
        &format!("{path}/editor/snapshots?limit=2"),
        None,
    )
    .await;
    assert_eq!(snapshots["total"], 3);
    assert_eq!(snapshots["items"].as_array().unwrap().len(), 2);
    assert_eq!(snapshots["next_offset"], 2);
    assert!(snapshots["items"][0].get("content").is_none());
    let id = conflict["snapshot"]["id"].as_str().unwrap();
    assert_eq!(
        call(
            &fixture.app,
            "GET",
            &format!("{path}/editor/snapshots/{id}"),
            None
        )
        .await
        .1,
        conflict["snapshot"]
    );
}
#[tokio::test]
async fn journal_commit_accepts_only_saved_id_and_returns_frozen_publication() {
    let fixture = setup().await;
    let operation = prepare_create(&fixture).await;
    let path = format!(
        "{}/{}",
        collection(&fixture, "workflow-operations"),
        operation["id"].as_str().unwrap()
    );
    assert_eq!(
        call(&fixture.app, "POST", &format!("{path}/acknowledge"), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    for body in [
        json!({"request":{}}),
        json!({"spec":{}}),
        json!({"rights":["all"]}),
        json!([]),
        Value::Null,
    ] {
        let (status, error) =
            call(&fixture.app, "POST", &format!("{path}/commit"), Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(
            call(&fixture.app, "GET", &path, None).await.1["state"],
            "pending"
        );
    }
    let (status, accepted) = call(&fixture.app, "POST", &format!("{path}/commit"), None).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["state"], "accepted");
    assert_eq!(accepted["receipt"]["kind"], "created");
    let publication = &accepted["receipt"]["publication"];
    let workflow = publication["workflow_id"].as_str().unwrap();
    let mut updated = definition(&fixture.profile);
    updated.name = "Later definition".into();
    fixture
        .runtime
        .store
        .publish_workflow(
            &fixture.project.id,
            workflow,
            1,
            "later-publication",
            &updated,
        )
        .await
        .unwrap();
    assert_eq!(
        call(
            &fixture.app,
            "POST",
            &format!("{path}/commit"),
            Some(json!({}))
        )
        .await
        .1,
        accepted
    );
    assert_eq!(
        call(&fixture.app, "POST", &format!("{path}/discard"), None)
            .await
            .1["state"],
        "accepted"
    );
    assert_eq!(publication["revision"], 1);
    let (_, ack) = call(&fixture.app, "POST", &format!("{path}/acknowledge"), None).await;
    assert!(ack["acknowledged_at"].is_i64());
    assert_eq!(
        call(
            &fixture.app,
            "GET",
            &collection(&fixture, "workflow-operations"),
            None
        )
        .await
        .1["total"],
        0
    );
    assert_eq!(
        call(
            &fixture.app,
            "GET",
            &format!(
                "{}?include_acknowledged=true",
                collection(&fixture, "workflow-operations")
            ),
            None
        )
        .await
        .1["total"],
        1
    );
}
#[tokio::test]
async fn journal_paths_snapshots_and_requests_keep_project_scope() {
    let fixture = setup().await;
    let (_, saved) = call(
        &fixture.app,
        "PUT",
        &collection(&fixture, "workflow-drafts"),
        Some(draft_body()),
    )
    .await;
    let operation = prepare_create(&fixture).await;
    let foreign = fixture
        .runtime
        .store
        .create_project(CreateProject {
            name: "Foreign project".into(),
        })
        .await
        .unwrap();
    let foreign_base = format!("/api/v2/projects/{}", foreign.id);
    for (method, suffix) in [
        ("GET", "workflow-drafts/editor".to_string()),
        ("GET", "workflow-drafts/editor/snapshots".into()),
        (
            "GET",
            format!(
                "workflow-drafts/editor/snapshots/{}",
                saved["snapshot"]["id"].as_str().unwrap()
            ),
        ),
        (
            "GET",
            format!("workflow-operations/{}", operation["id"].as_str().unwrap()),
        ),
        (
            "POST",
            format!(
                "workflow-operations/{}/commit",
                operation["id"].as_str().unwrap()
            ),
        ),
        (
            "POST",
            format!(
                "workflow-operations/{}/discard",
                operation["id"].as_str().unwrap()
            ),
        ),
        (
            "POST",
            format!(
                "workflow-operations/{}/acknowledge",
                operation["id"].as_str().unwrap()
            ),
        ),
    ] {
        assert_eq!(
            call(
                &fixture.app,
                method,
                &format!("{foreign_base}/{suffix}"),
                None
            )
            .await
            .0,
            StatusCode::NOT_FOUND,
            "{suffix}"
        );
    }
    let library = create(&fixture).await;
    let request = json!({"intent_id":"foreign-start","request":{"kind":"start","request":start(&fixture,&library)}});
    assert_eq!(
        call(
            &fixture.app,
            "POST",
            &format!("{foreign_base}/workflow-operations"),
            Some(request)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let mut command = json!({"intent_id":"foreign-snapshot","request":{"kind":"create","request":{"idempotency_key":"foreign-create","definition":definition(&fixture.profile)}}});
    command["draft_snapshot"] = json!({"draft_id":"editor","snapshot_id":saved["snapshot"]["id"],"sha256":saved["snapshot"]["sha256"]});
    assert_eq!(
        call(
            &fixture.app,
            "POST",
            &format!("{foreign_base}/workflow-operations"),
            Some(command)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &fixture.app,
            "GET",
            &format!("{foreign_base}/workflow-operations"),
            None
        )
        .await
        .1["total"],
        0
    );
}
#[tokio::test]
async fn journal_wire_limits_and_unknown_authority_are_rejected_without_effects() {
    let fixture = setup().await;
    let drafts = collection(&fixture, "workflow-drafts");
    let ops = collection(&fixture, "workflow-operations");
    let mut body = draft_body();
    body["grant"] = json!({"autonomous":true});
    assert_eq!(
        call(&fixture.app, "PUT", &drafts, Some(body)).await.0,
        StatusCode::BAD_REQUEST
    );
    let library = create(&fixture).await;
    for field in [
        "spec",
        "max_parallel",
        "allowed_tools",
        "folders",
        "trusted_grant",
    ] {
        let mut input = serde_json::to_value(start(&fixture, &library)).unwrap();
        input[field] = json!("untrusted");
        let body =
            json!({"intent_id":"unsupported-authority","request":{"kind":"start","request":input}});
        assert_eq!(
            call(&fixture.app, "POST", &ops, Some(body)).await.0,
            StatusCode::BAD_REQUEST,
            "{field}"
        );
    }
    for query in ["limit=0", "limit=101", "offset=4294967295", "extra=true"] {
        assert!(
            call(&fixture.app, "GET", &format!("{drafts}?{query}"), None)
                .await
                .0
                .is_client_error(),
            "{query}"
        );
        assert!(
            call(&fixture.app, "GET", &format!("{ops}?{query}"), None)
                .await
                .0
                .is_client_error(),
            "{query}"
        );
    }
    let mut big = draft_body();
    big["content"]["data"] = json!("x".repeat(1_100_100));
    assert!(
        call(&fixture.app, "PUT", &drafts, Some(big))
            .await
            .0
            .is_client_error()
    );
    // Application-specific draft cap remains stricter than the unchanged HTTP cap.
    let mut big = draft_body();
    big["content"]["data"] = json!("x".repeat(1_000_100));
    assert_eq!(
        call(&fixture.app, "PUT", &drafts, Some(big)).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(call(&fixture.app, "GET", &drafts, None).await.1["total"], 0);
    assert_eq!(call(&fixture.app, "GET", &ops, None).await.1["total"], 0);
}
#[tokio::test]
async fn journal_prepared_start_rejects_changed_original_request_and_direct_races() {
    let fixture = setup().await;
    let library = create(&fixture).await;
    let ops = collection(&fixture, "workflow-operations");
    let body = json!({"intent_id":"start-intent","request":{"kind":"start","request":start(&fixture,&library)}});
    let (status, operation) = call(&fixture.app, "POST", &ops, Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(&fixture.app, "POST", &ops, Some(body.clone())).await.1,
        operation
    );
    let mut changed = body;
    changed["request"]["request"]["base_revision"] = json!(2);
    assert_eq!(
        call(&fixture.app, "POST", &ops, Some(changed)).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &fixture.app,
            "POST",
            "/api/v2/runs",
            Some(serde_json::to_value(start(&fixture, &library)).unwrap())
        )
        .await
        .0,
        StatusCode::CONFLICT
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
    let path = format!("{}/{}", ops, operation["id"].as_str().unwrap());
    let (status, accepted) = call(&fixture.app, "POST", &format!("{path}/commit"), None).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["state"], "accepted");
    assert_eq!(accepted["receipt"]["kind"], "started");
    assert!(accepted["receipt"]["spec"]["provider"].is_null());
    assert_eq!(accepted["receipt"]["spec"]["folders"], json!([]));
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
async fn journal_commit_transient_errors_leave_pending_and_deterministic_errors_return_rejected() {
    let fixture = setup().await;
    let library = create(&fixture).await;
    let ops = collection(&fixture, "workflow-operations");
    let (_,operation)=call(&fixture.app,"POST",&ops,Some(json!({"intent_id":"pending-provider","request":{"kind":"start","request":start(&fixture,&library)}}))).await;
    let path = format!("{}/{}", ops, operation["id"].as_str().unwrap());
    let configured = std::mem::take(&mut *fixture.runtime.providers.write().await);
    let (status, error) = call(&fixture.app, "POST", &format!("{path}/commit"), None).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{error}");
    assert_eq!(error["code"], "provider_unavailable");
    assert!(error["correlation_id"].is_string());
    assert_eq!(
        call(&fixture.app, "GET", &path, None).await.1["state"],
        "pending"
    );
    *fixture.runtime.providers.write().await = configured;
    assert_eq!(
        call(&fixture.app, "POST", &format!("{path}/commit"), None)
            .await
            .1["state"],
        "accepted"
    );
    let mut invalid = definition(&fixture.profile);
    invalid.steps[0].role_id = "missing-role".into();
    let (_,operation)=call(&fixture.app,"POST",&ops,Some(json!({"intent_id":"invalid-definition","request":{"kind":"publish","workflow_id":library["id"],"request":{"base_revision":1,"idempotency_key":"invalid-publish","definition":invalid}}}))).await;
    let path = format!("{}/{}", ops, operation["id"].as_str().unwrap());
    let (status, rejected) = call(&fixture.app, "POST", &format!("{path}/commit"), None).await;
    assert_eq!(status, StatusCode::OK, "{rejected}");
    assert_eq!(rejected["state"], "rejected");
    assert_eq!(rejected["rejection"]["code"], "invalid_request");
    assert_eq!(rejected["request"], operation["request"]);
    assert_eq!(
        call(&fixture.app, "POST", &format!("{path}/commit"), None)
            .await
            .1,
        rejected
    );
}
