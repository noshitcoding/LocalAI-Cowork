#![allow(dead_code)] // Shared by storage and application-service integration suites.
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use cowork_contracts::*;
use cowork_domain::{mcp::*, *};
use cowork_mcp::{Endpoint, Limits, PreparedCall, Session};
use cowork_storage::{Change, Store, id, now};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{io::AsyncWriteExt, task::JoinHandle};

pub fn definition() -> Value {
    json!({"name":"append_note","inputSchema":{"type":"object","properties":{"text":{"type":"string","minLength":1}},"required":["text"],"additionalProperties":false},
        "outputSchema":{"type":"object","properties":{"written":{"type":"boolean"}},"required":["written"]},
        "annotations":{"readOnlyHint":true,"idempotentHint":true}})
}
pub fn response() -> Value {
    json!({"content":[{"type":"text","text":"recorded"}],"structuredContent":{"written":true}})
}
pub fn result() -> McpResult {
    let response = response();
    McpResult {
        sha256: canonical_hash(&response),
        response,
    }
}
pub struct RemoteState {
    pub required_bearer: Mutex<Option<String>>,
    pub initializations: AtomicUsize,
    pub initialize_delay_ms: AtomicU64,
    pub session_ids: Mutex<Vec<String>>,
    pub used_sessions: Mutex<Vec<String>>,
    pub path: PathBuf,
    pub calls: Mutex<Vec<Value>>,
    pub delay: Duration,
    pub tool_error: bool,
    pub durable_effects: AtomicUsize,
}
pub struct Remote {
    pub endpoint: String,
    pub state: Arc<RemoteState>,
    task: JoinHandle<()>,
}
impl Drop for Remote {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Remote {
    pub async fn new(path: PathBuf, delay: Duration, tool_error: bool) -> Self {
        let state = Arc::new(RemoteState {
            required_bearer: Mutex::new(None),
            initializations: AtomicUsize::new(0),
            initialize_delay_ms: AtomicU64::new(0),
            session_ids: Mutex::new(vec![]),
            used_sessions: Mutex::new(vec![]),
            path,
            calls: Mutex::new(vec![]),
            delay,
            tool_error,
            durable_effects: AtomicUsize::new(0),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/mcp", post(handle))
            .with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            endpoint,
            state,
            task,
        }
    }
    pub async fn session(&self) -> Session {
        session(&self.endpoint).await
    }
    pub fn calls(&self) -> usize {
        self.state.calls.lock().unwrap().len()
    }
}
pub async fn session(endpoint: &str) -> Session {
    let mut session = Session::connect(
        Endpoint::new(endpoint, None).unwrap(),
        Limits {
            timeout_ms: 2000,
            ..Limits::default()
        },
    )
    .await
    .unwrap();
    session.discover().await.unwrap();
    session
}
async fn handle(
    State(state): State<Arc<RemoteState>>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    if state
        .required_bearer
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|token| {
            headers.get("authorization").and_then(|v| v.to_str().ok())
                != Some(format!("Bearer {token}").as_str())
        })
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if request["method"] != "initialize"
        && !headers
            .get("Mcp-Session-Id")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| state.session_ids.lock().unwrap().iter().any(|s| s == h))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let result = match request["method"].as_str() {
        Some("initialize") => {
            let session_id = format!("synthetic-secret-{}", id());
            state.session_ids.lock().unwrap().push(session_id.clone());
            state.initializations.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(
                state.initialize_delay_ms.load(Ordering::SeqCst),
            ))
            .await;
            let result = json!({"protocolVersion":cowork_mcp::PROTOCOL_VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"synthetic","version":"1"}});
            let mut response =
                Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result})).into_response();
            response.headers_mut().insert(
                "Mcp-Session-Id",
                HeaderValue::from_str(&session_id).unwrap(),
            );
            return response;
        }
        Some("notifications/initialized") => return StatusCode::ACCEPTED.into_response(),
        Some("tools/list") => json!({"tools":[definition()]}),
        Some("tools/call") => {
            state
                .used_sessions
                .lock()
                .unwrap()
                .push(headers["Mcp-Session-Id"].to_str().unwrap().into());
            state.calls.lock().unwrap().push(request.clone());
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&state.path)
                .await
                .unwrap();
            file.write_all(b"effect\n").await.unwrap();
            file.sync_all().await.unwrap();
            drop(file);
            state.durable_effects.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(state.delay).await;
            if state.tool_error {
                json!({"isError":true,"content":[{"type":"text","text":"partial mutation"}]})
            } else {
                response()
            }
        }
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result})).into_response()
}
pub struct Fixture {
    pub run: RunRecord,
    pub token: String,
    pub tool: ToolCall,
    pub server: McpServerRecord,
}
pub async fn setup(store: &Store, remote: &Remote, mode: AccessMode) -> Fixture {
    setup_with_credential(store, remote, mode, None).await
}
pub async fn setup_with_credential(
    store: &Store,
    remote: &Remote,
    mode: AccessMode,
    secret: Option<(&dyn cowork_secrets::KeyStore, cowork_secrets::KeyId, &str)>,
) -> Fixture {
    let project = store
        .create_project(CreateProject {
            name: "Synthetic MCP".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "MCP journal".into(),
        })
        .await
        .unwrap();
    let server_id = id();
    let reference = if let Some((keys, key, bearer)) = secret {
        let scope = cowork_secrets::CredentialScope {
            project_id: project.id.clone(),
            server_id: server_id.clone(),
            reference: id(),
            revision: 1,
            endpoint: remote.endpoint.clone(),
        };
        let sealed = cowork_secrets::seal_bearer(keys, key, &scope, bearer).unwrap();
        store.put_mcp_credential(&scope, &sealed, 0).await.unwrap();
        Some(scope.reference)
    } else {
        None
    };
    let revision = if reference.is_some() { 1 } else { 0 };
    let server = store
        .configure_mcp_server(
            &project.id,
            &server_id,
            0,
            "create",
            &McpServerConfiguration {
                name: "Synthetic endpoint".into(),
                endpoint: remote.endpoint.clone(),
                credential_reference: reference,
                credential_revision: revision,
                catalog: vec![definition()],
                selected_tools: vec!["append_note".into()],
                enabled: true,
            },
        )
        .await
        .unwrap();
    let binding = server.bindings().unwrap().remove(0);
    let tool = ToolCall {
        id: id(),
        name: binding.alias.clone(),
        arguments: json!({"text":"exact input"}),
    };
    let spec = RunSpec {
        input_workspace: None,
        agent_config: None,
        model_binding: None,
        output: None,
        workflow: None,
        mcp_tools: vec![binding],
        environment: None,
        order_id: order.id.clone(),
        project_id: project.id.clone(),
        prompt: "Synthetic MCP step".into(),
        provider: Some(ProviderProfile {
            generation: Default::default(),
            id: "fixture".into(),
            name: "unused".into(),
            kind: ProviderKind::Test,
            endpoint: String::new(),
            model: "test".into(),
            revision: 1,
            context_tokens: 32768,
            credential_required: false,
            credential_ready: true,
            capabilities: Default::default(),
        }),
        file_action: None,
        access_mode: mode.clone(),
        executor: "local-v2".into(),
        policy_revision: 1,
        allowed_tools: vec![tool.name.clone()],
        budgets: Budgets::default(),
        folders: vec![FolderGrant {
            id: id(),
            project_id: project.id,
            name: "Own workspace".into(),
            writable: true,
            revision: "fixture".into(),
            resource_id: id(),
            order_id: Some(order.id.clone()),
        }],
    };
    let input = CreateRun {
        agent_config: None,
        model_binding: None,
        output: None,
        native_tools: vec![],
        environment: None,
        mcp_tools: vec![McpToolSelection {
            server_id: server.id.clone(),
            server_revision: server.revision,
            tool_name: "append_note".into(),
        }],
        order_id: order.id,
        base_revision: order.revision,
        idempotency_key: id(),
        prompt: spec.prompt.clone(),
        provider_id: "fixture".into(),
        provider_revision: 1,
        access_mode: mode,
        budgets: Budgets::default(),
        folders: vec![],
    };
    let run = store.create_run(&input, spec).await.unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    assert_eq!(claim.record.id, run.id);
    let token = claim.lease.unwrap().token;
    store
        .mutate(&run.id, Some(&token), |r| {
            r.checkpoint.pending_tools = vec![tool.clone()];
            Ok(Change::default())
        })
        .await
        .unwrap();
    Fixture {
        run,
        token,
        tool,
        server,
    }
}
pub fn prepare(session: &Session, f: &Fixture) -> PreparedCall {
    session
        .prepare(
            &f.server.catalog_sha256,
            "append_note",
            f.tool.arguments.clone(),
        )
        .unwrap()
}
pub async fn approve(store: &Store, f: &mut Fixture) {
    let approval = Approval {
        id: id(),
        run_id: f.run.id.clone(),
        tool: f.tool.clone(),
        target: f.run.spec.executor.clone(),
        policy_revision: f.run.spec.policy_revision,
        fingerprint: approval_fingerprint(&f.run, &f.tool),
        decision: None,
    };
    store
        .mutate(&f.run.id, Some(&f.token), |r| {
            r.checkpoint.approvals.push(approval.clone());
            r.transition(RunState::WaitingApproval)?;
            r.lease = None;
            Ok(Change::event(
                "approval_requested",
                json!({"approval":approval}),
            ))
        })
        .await
        .unwrap();
    store
        .resolve_approval(
            &f.run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint,
                allow: true,
            },
        )
        .await
        .unwrap();
    f.token = store.claim().await.unwrap().unwrap().lease.unwrap().token;
}
pub async fn expire(store: &Store, f: &Fixture) {
    store
        .mutate(&f.run.id, None, |r| {
            r.lease.as_mut().unwrap().until = now() - 1;
            Ok(Change::default())
        })
        .await
        .unwrap();
    assert_eq!(store.recover_expired_leases().await.unwrap(), 1);
}
