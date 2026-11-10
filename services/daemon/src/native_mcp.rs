//! A native-confirmed stdio factory. The HTTP bridge credential never reaches the
//! renderer or child environment. Each initialized connection owns its process tree.
use super::*;
use axum::{Extension, http::HeaderMap};
use cowork_storage::{NativeMcpFactory, RevisionRequest};
use cowork_toolbox::Toolbox;
use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

#[derive(Default)]
pub(super) struct StdioBridges {
    entries: Mutex<BTreeMap<String, Arc<Bridge>>>,
}
struct Bridge {
    token: zeroize::Zeroizing<String>,
    revision: i64,
    program: String,
    arguments: Vec<String>,
    cwd: Option<String>,
    environment: BTreeMap<String, String>,
    native_only: bool,
    closed: AtomicBool,
    connections: Mutex<BTreeMap<String, Arc<Connection>>>,
}
struct Connection {
    process: String,
    busy: AtomicBool,
    last_used: Mutex<Instant>,
}
type Configuration = ConfigureOwnerMcpStdio;
type Credential = ConfigureOwnerMcpCredential;
impl Drop for Bridge {
    fn drop(&mut self) {
        for value in self.environment.values_mut() {
            zeroize::Zeroize::zeroize(value);
        }
    }
}
/// Mounted only below the ordinary owner bearer/origin middleware. Actor sessions
/// continue to use their existing private/team routes and cannot reach this owner API.
pub(super) fn owner_router(toolbox: Arc<Toolbox>, bridges: Arc<StdioBridges>) -> Router<AppState> {
    Router::new()
        .route("/mcp/capabilities", get(server_capabilities))
        .route("/mcp/credential", post(server_credential))
        .route(
            "/mcp/credential/{project}/{server}",
            get(server_credential_status),
        )
        .route("/mcp/discovery", post(server_discovery))
        .route("/mcp/stdio", post(server_configure))
        .route("/mcp/stdio/{project}", get(server_factories))
        .route(
            "/mcp/stdio/{project}/{server}/disconnect",
            post(server_disconnect),
        )
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(Extension(toolbox))
        .layer(Extension(bridges))
}
pub(super) fn server_bridge_origin(state: &AppState) -> Option<String> {
    // server_config::origins places the configured daemon loopback origin third.
    let origin = state.origins.get(2)?;
    let url = reqwest::Url::parse(origin).ok()?;
    (url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port().is_some()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| origin.clone())
}
fn server_allowed(state: &AppState) -> std::result::Result<(), Error> {
    if state.native_token.is_some() || !state.runtime.server_owner_mcp_available() {
        return Err(Error::Forbidden);
    }
    Ok(())
}
async fn server_capabilities(
    State(state): State<AppState>,
    Extension(toolbox): Extension<Arc<Toolbox>>,
) -> Answer<Value> {
    let owner = server_allowed(&state).is_ok();
    let programs = toolbox.availability().programs;
    Ok(Json(
        json!({"http":owner,"stdio":owner&&programs.iter().any(|name|name=="python")&&server_bridge_origin(&state).is_some(),"programs":programs,"stdio_cwd_policy":"managed_relative","environment":true}),
    ))
}
async fn server_credential(
    State(state): State<AppState>,
    body: Input<Credential>,
) -> Answer<Value> {
    server_allowed(&state)?;
    let input = input(body)?;
    if state
        .runtime
        .store
        .is_connector_server(&input.server_id)
        .await?
    {
        return Err(Error::Forbidden.into());
    }
    let (reference, revision) = state
        .runtime
        .save_server_mcp_bearer(
            &input.project_id,
            &input.server_id,
            &input.endpoint,
            input.bearer,
            input.expected_revision,
        )
        .await?;
    Ok(Json(
        json!({"credential_reference":reference,"credential_revision":revision}),
    ))
}
async fn server_credential_status(
    State(state): State<AppState>,
    Path((project, server)): Path<(String, String)>,
) -> Answer<Value> {
    server_allowed(&state)?;
    let saved = state
        .runtime
        .store
        .mcp_credential(&project, &format!("server-mcp:{server}"))
        .await?;
    if saved.revoked || saved.scope.server_id != server {
        return Err(Error::NotFound.into());
    }
    Ok(Json(
        json!({"endpoint":saved.scope.endpoint,"credential_reference":saved.scope.reference,"credential_revision":saved.revision}),
    ))
}
type ServerDiscovery = OwnerMcpDiscovery;
async fn server_discovery(
    State(state): State<AppState>,
    body: Input<ServerDiscovery>,
) -> Answer<Value> {
    server_allowed(&state)?;
    let request = input(body)?;
    state
        .runtime
        .store
        .projects()
        .await?
        .into_iter()
        .find(|project| project.id == request.project_id)
        .ok_or(Error::NotFound)?;
    uuid::Uuid::parse_str(&request.server_id).map_err(|_| Error::Forbidden)?;
    let bearer = if let Some(reference) = request.credential_reference {
        if reference != format!("server-mcp:{}", request.server_id)
            || request.credential_revision <= 0
        {
            return Err(Error::Forbidden.into());
        }
        Some(
            state
                .runtime
                .native_mcp_bearer(
                    &request.project_id,
                    &request.server_id,
                    &request.endpoint,
                    &reference,
                    request.credential_revision,
                )
                .await?,
        )
    } else {
        if request.credential_revision != 0 {
            return Err(Error::Forbidden.into());
        }
        None
    };
    let endpoint = cowork_mcp::Endpoint::new(
        &request.endpoint,
        bearer.as_ref().map(|value| value.as_str()),
    )
    .map_err(|_| Error::Validation("Invalid MCP endpoint".into()))?;
    let result = tokio::time::timeout(Duration::from_secs(60), async {
        let session = state
            .runtime
            .connect_owner_mcp_catalog(
                &request.server_id,
                endpoint.url(),
                bearer.as_deref().map(String::as_str),
                Default::default(),
                None,
                false,
            )
            .await?;
        let catalog = session.catalog().ok_or(Error::McpSessionUnavailable)?;
        let definitions: Vec<Value> = catalog
            .tools()
            .map(|(_, tool)| tool.definition().clone())
            .collect();
        Ok::<_, Error>(json!({"catalog":definitions,"catalog_sha256":catalog.sha256()}))
    })
    .await
    .map_err(|_| Error::McpSessionUnavailable)??;
    Ok(Json(result))
}
async fn server_configure(
    State(state): State<AppState>,
    Extension(bridges): Extension<Arc<StdioBridges>>,
    Extension(toolbox): Extension<Arc<Toolbox>>,
    body: Input<Configuration>,
) -> Answer<Value> {
    server_allowed(&state)?;
    if !toolbox
        .availability()
        .programs
        .iter()
        .any(|name| name == "python")
    {
        return Err(
            Error::Validation("Server stdio requires a configured Python runtime".into()).into(),
        );
    }
    configure_inner(state, bridges, toolbox, input(body)?, false).await
}
async fn server_factories(
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Answer<Vec<NativeMcpFactory>> {
    server_allowed(&state)?;
    Ok(Json(
        state
            .runtime
            .store
            .native_mcp_factories(&project)
            .await?
            .into_iter()
            .filter(|factory| !factory.native_only)
            .collect(),
    ))
}
async fn server_disconnect(
    State(state): State<AppState>,
    Path((project, server)): Path<(String, String)>,
    Extension(bridges): Extension<Arc<StdioBridges>>,
    Extension(toolbox): Extension<Arc<Toolbox>>,
    body: Input<RevisionRequest>,
) -> Answer<Value> {
    server_allowed(&state)?;
    disconnect_inner(
        state,
        project,
        server,
        bridges,
        toolbox,
        input(body)?.expected_revision,
        false,
    )
    .await
}
async fn save_bridge_token(
    state: &AppState,
    project: &str,
    server: &str,
    endpoint: &str,
    token: String,
    expected: i64,
    native: bool,
) -> std::result::Result<(String, i64), Error> {
    if native {
        state
            .runtime
            .save_native_mcp_bearer(project, server, endpoint, token, expected)
            .await
    } else {
        state
            .runtime
            .save_server_mcp_bearer(project, server, endpoint, token, expected)
            .await
    }
}
pub(super) fn native_router(
    state: AppState,
    toolbox: Arc<Toolbox>,
    bridges: Arc<StdioBridges>,
) -> Router<AppState> {
    Router::new()
        .route("/mcp/stdio", post(configure))
        .route("/mcp/credential", post(credential))
        .route("/mcp/credential/{project}/{server}", get(credential_status))
        .route("/mcp/stdio/{project}", get(factories))
        .route("/mcp/stdio/{project}/{server}/disconnect", post(disconnect))
        .layer(Extension(toolbox))
        .layer(Extension(bridges))
        .layer(middleware::from_fn_with_state(
            state,
            native_folders::authorize_native,
        ))
}
async fn credential_status(
    State(state): State<AppState>,
    Path((project, server)): Path<(String, String)>,
) -> Answer<Value> {
    state
        .runtime
        .store
        .projects()
        .await?
        .into_iter()
        .find(|p| p.id == project)
        .ok_or(Error::NotFound)?;
    let saved = state
        .runtime
        .store
        .mcp_credential(&project, &format!("owner-mcp:{server}"))
        .await?;
    if saved.revoked || saved.scope.server_id != server {
        return Err(Error::NotFound.into());
    }
    Ok(Json(
        json!({"endpoint":saved.scope.endpoint,"credential_reference":saved.scope.reference,"credential_revision":saved.revision}),
    ))
}
pub(super) fn bridge_router(toolbox: Arc<Toolbox>, bridges: Arc<StdioBridges>) -> Router<AppState> {
    Router::new()
        .route("/native-mcp/{bridge}", post(rpc).delete(close))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(Extension(toolbox))
        .layer(Extension(bridges))
}
async fn credential(State(state): State<AppState>, body: Input<Credential>) -> Answer<Value> {
    let input = input(body)?;
    if state
        .runtime
        .store
        .is_connector_server(&input.server_id)
        .await?
    {
        return Err(Error::Forbidden.into());
    }
    let (reference, revision) = state
        .runtime
        .save_native_mcp_bearer(
            &input.project_id,
            &input.server_id,
            &input.endpoint,
            input.bearer,
            input.expected_revision,
        )
        .await?;
    Ok(Json(
        json!({"credential_reference":reference,"credential_revision":revision}),
    ))
}
async fn configure(
    State(state): State<AppState>,
    Extension(bridges): Extension<Arc<StdioBridges>>,
    Extension(toolbox): Extension<Arc<Toolbox>>,
    body: Input<Configuration>,
) -> Answer<Value> {
    configure_inner(state, bridges, toolbox, input(body)?, true).await
}
async fn configure_inner(
    state: AppState,
    bridges: Arc<StdioBridges>,
    toolbox: Arc<Toolbox>,
    request: Configuration,
    native: bool,
) -> Answer<Value> {
    state
        .runtime
        .store
        .projects()
        .await?
        .into_iter()
        .find(|p| p.id == request.project_id)
        .ok_or(Error::NotFound)?;
    uuid::Uuid::parse_str(&request.server_id).map_err(|_| Error::Forbidden)?;
    if request.program.is_empty()
        || request.program.len() > 128
        || request.arguments.len() > 128
        || request.arguments.iter().map(String::len).sum::<usize>() > 32768
        || request
            .arguments
            .iter()
            .any(|s| s.len() > 8192 || s.contains('\0'))
        || request.expected_revision < 0
    {
        return Err(Error::Forbidden.into());
    }
    if !toolbox.availability().programs.contains(&request.program) {
        return Err(Error::Validation("Select a configured executable alias".into()).into());
    }
    if state
        .runtime
        .store
        .is_connector_server(&request.server_id)
        .await?
    {
        return Err(Error::Forbidden.into());
    }
    let id = request.server_id.clone();
    if let Some(environment) = &request.environment {
        cowork_toolbox::validate_stdio_environment(environment)
            .map_err(|_| Error::Validation("Invalid MCP environment".into()))?;
    }
    toolbox
        .stdio_directory(&id, request.cwd.as_deref(), native)
        .map_err(|_| Error::Validation("Invalid MCP working directory".into()))?;
    let origin = if native {
        "http://127.0.0.1:43821".to_owned()
    } else {
        server_bridge_origin(&state).ok_or(Error::Forbidden)?
    };
    let endpoint = format!("{origin}/native-mcp/{id}");
    let old = state.runtime.store.native_mcp_factory(&id).await?;
    if old.as_ref().is_some_and(|f| {
        f.project_id != request.project_id
            || f.native_only != native
            || f.revision != request.expected_revision
                && !(request.expected_revision.checked_add(1) == Some(f.revision)
                    && f.enabled
                    && f.program == request.program
                    && f.arguments == request.arguments
                    && f.cwd == request.cwd)
    }) {
        return Err(Error::Conflict.into());
    }
    if let Some(previous) = &old
        && previous.revision != request.expected_revision
        && let Some(environment) = &request.environment
        && state.runtime.mcp_stdio_environment(previous).await? != *environment
    {
        return Err(Error::Conflict.into());
    }
    let reference = format!("{}-mcp:{id}", if native { "owner" } else { "server" });
    let (reference, revision, token) = match state
        .runtime
        .store
        .mcp_credential(&request.project_id, &reference)
        .await
    {
        Ok(saved) => {
            // Changing the factory rotates its bridge credential, invalidating old run bindings.
            let changed = old.as_ref().is_some_and(|factory| {
                factory.revision == request.expected_revision
                    && saved.revision == factory.credential_revision
            });
            if changed {
                let token = format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                );
                let (reference, revision) = save_bridge_token(
                    &state,
                    &request.project_id,
                    &id,
                    &endpoint,
                    token.clone(),
                    saved.revision,
                    native,
                )
                .await?;
                (reference, revision, zeroize::Zeroizing::new(token))
            } else {
                let token = state
                    .runtime
                    .native_mcp_bearer(
                        &request.project_id,
                        &id,
                        &endpoint,
                        &reference,
                        saved.revision,
                    )
                    .await?;
                (reference, saved.revision, token)
            }
        }
        Err(Error::NotFound) => {
            let token = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            let (reference, revision) = save_bridge_token(
                &state,
                &request.project_id,
                &id,
                &endpoint,
                token.clone(),
                0,
                native,
            )
            .await?;
            (reference, revision, zeroize::Zeroizing::new(token))
        }
        Err(error) => return Err(error.into()),
    };
    let environment = if let Some(environment) = &request.environment {
        state
            .runtime
            .save_mcp_stdio_environment(&request.project_id, &id, &endpoint, environment, native)
            .await?
    } else {
        old.as_ref()
            .map(|f| f.environment.clone())
            .unwrap_or_default()
    };
    let factory = NativeMcpFactory {
        server_id: id.clone(),
        project_id: request.project_id,
        program: request.program.clone(),
        arguments: request.arguments.clone(),
        cwd: request.cwd.clone(),
        environment,
        native_only: native,
        endpoint: endpoint.clone(),
        credential_reference: reference.clone(),
        credential_revision: revision,
        revision: request
            .expected_revision
            .checked_add(1)
            .ok_or(Error::Conflict)?,
        enabled: true,
    };
    let factory = if old.as_ref().is_some_and(|f| {
        f.revision == factory.revision
            && f.enabled
            && f.program == factory.program
            && f.arguments == factory.arguments
            && f.cwd == factory.cwd
            && f.environment == factory.environment
            && f.credential_revision == factory.credential_revision
    }) {
        old.unwrap()
    } else {
        state
            .runtime
            .store
            .put_native_mcp_factory(&factory, request.expected_revision)
            .await?
    };
    let environment = state.runtime.mcp_stdio_environment(&factory).await?;
    remove_connections(&bridges, &toolbox, &id).await?;
    {
        let mut entries = bridges.entries.lock().map_err(|_| Error::Conflict)?;
        entries.retain(|_, b| {
            let keep = !b
                .connections
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty();
            if !keep {
                b.closed.store(true, Ordering::Release);
            }
            keep
        });
        if entries.len() >= 64 {
            return Err(Error::Conflict.into());
        }
        entries.insert(
            id.clone(),
            Arc::new(Bridge {
                token: token.clone(),
                revision: factory.revision,
                program: request.program,
                arguments: request.arguments,
                cwd: factory.cwd.clone(),
                environment,
                native_only: native,
                closed: AtomicBool::new(false),
                connections: Mutex::new(BTreeMap::new()),
            }),
        );
    }
    let discovered=tokio::time::timeout(Duration::from_secs(60),async{
        let session=state.runtime.connect_owner_mcp_catalog(&id,&endpoint,Some(&token),Default::default(),None,false).await?;
        let catalog=session.catalog().ok_or(Error::McpSessionUnavailable)?;
        let definitions:Vec<Value>=catalog.tools().map(|(_,t)|t.definition().clone()).collect();
        Ok::<_,Error>(json!({"endpoint":endpoint,"catalog":definitions,"catalog_sha256":catalog.sha256(),"credential_reference":reference,"credential_revision":revision,"factory_revision":factory.revision}))
    }).await;
    remove_connections(&bridges, &toolbox, &id).await?;
    Ok(Json(discovered.map_err(|_| Error::McpSessionUnavailable)??))
}
async fn authenticated(
    state: &AppState,
    bridges: &StdioBridges,
    toolbox: &Arc<Toolbox>,
    id: &str,
    headers: &HeaderMap,
) -> std::result::Result<Arc<Bridge>, ApiFailure> {
    if headers.contains_key(header::ORIGIN) {
        return Err(Error::Forbidden.into());
    }
    let factory = state
        .runtime
        .store
        .native_mcp_factory(id)
        .await?
        .filter(|f| f.enabled)
        .ok_or(Error::NotFound)?;
    let token = state
        .runtime
        .native_mcp_bearer(
            &factory.project_id,
            &factory.server_id,
            &factory.endpoint,
            &factory.credential_reference,
            factory.credential_revision,
        )
        .await?;
    if bridges
        .entries
        .lock()
        .map_err(|_| Error::Conflict)?
        .get(id)
        .is_some_and(|b| b.revision > factory.revision)
    {
        return Err(Error::Conflict.into());
    }
    let obsolete = bridges
        .entries
        .lock()
        .map_err(|_| Error::Conflict)?
        .get(id)
        .is_some_and(|b| b.revision != factory.revision || b.token.as_str() != token.as_str());
    if obsolete {
        remove_connections(bridges, toolbox, id).await?;
    }
    let cached = bridges
        .entries
        .lock()
        .map_err(|_| Error::Conflict)?
        .get(id)
        .cloned();
    if let Some(bridge) = cached {
        if bridge.closed.load(Ordering::Acquire) {
            return Err(Error::McpSessionUnavailable.into());
        }
        let expected = zeroize::Zeroizing::new(format!("Bearer {}", bridge.token.as_str()));
        if headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some(expected.as_str())
        {
            return Err(Error::Forbidden.into());
        }
        return Ok(bridge);
    }
    let environment = state.runtime.mcp_stdio_environment(&factory).await?;
    let bridge = {
        let mut entries = bridges.entries.lock().map_err(|_| Error::Conflict)?;
        entries.retain(|key, b| {
            let keep = key == id
                || !b
                    .connections
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_empty();
            if !keep {
                b.closed.store(true, Ordering::Release);
            }
            keep
        });
        if !entries.contains_key(id) && entries.len() >= 64 {
            return Err(Error::Conflict.into());
        }
        entries
            .entry(id.into())
            .or_insert_with(|| {
                Arc::new(Bridge {
                    token,
                    revision: factory.revision,
                    program: factory.program,
                    arguments: factory.arguments,
                    cwd: factory.cwd,
                    environment,
                    native_only: factory.native_only,
                    closed: AtomicBool::new(false),
                    connections: Mutex::new(BTreeMap::new()),
                })
            })
            .clone()
    };
    let expected = zeroize::Zeroizing::new(format!("Bearer {}", bridge.token.as_str()));
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some(expected.as_str())
    {
        return Err(Error::Forbidden.into());
    }
    Ok(bridge)
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
async fn rpc(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Extension(toolbox): Extension<Arc<Toolbox>>,
    Extension(bridges): Extension<Arc<StdioBridges>>,
    body: Input<Value>,
) -> std::result::Result<Response, ApiFailure> {
    let bridge = authenticated(&state, &bridges, &toolbox, &id, &headers).await?;
    let body = input(body)?;
    if body["jsonrpc"] != "2.0" || !body["method"].is_string() || !body.is_object() {
        return Err(Error::Forbidden.into());
    }
    let initialize = body["method"] == "initialize";
    let cancel = Arc::new(AtomicBool::new(false));
    let _guard = CancelOnDrop(cancel.clone());
    let (session, connection) = if initialize {
        if headers.contains_key("Mcp-Session-Id") {
            return Err(Error::Conflict.into());
        }
        if body.get("id").is_none() {
            return Err(Error::Forbidden.into());
        }
        let session = uuid::Uuid::new_v4().to_string();
        let process = uuid::Uuid::new_v4().to_string();
        let service = toolbox.clone();
        let owner = bridge.clone();
        tokio::task::spawn_blocking(move || {
            let expired = {
                let mut connections = owner.connections.lock().unwrap_or_else(|e| e.into_inner());
                let keys = connections
                    .iter()
                    .filter(|(_, c)| {
                        !c.busy.load(Ordering::Acquire)
                            && (c
                                .last_used
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .elapsed()
                                > Duration::from_secs(900)
                                || !service.persistent_alive(&c.process))
                    })
                    .map(|(s, _)| s.clone())
                    .collect::<Vec<_>>();
                keys.into_iter()
                    .filter_map(|s| connections.remove(&s))
                    .collect::<Vec<_>>()
            };
            for c in expired {
                let _ = service.stop_persistent(&c.process);
            }
        })
        .await
        .map_err(|_| Error::UncertainEffect)?;
        let connection = Arc::new(Connection {
            process,
            busy: AtomicBool::new(true),
            last_used: Mutex::new(Instant::now()),
        });
        {
            let mut connections = bridge.connections.lock().map_err(|_| Error::Conflict)?;
            if bridge.closed.load(Ordering::Acquire) || connections.len() >= 16 {
                return Err(Error::Conflict.into());
            }
            connections.insert(session.clone(), connection.clone());
        }
        (session, connection)
    } else {
        let session = headers
            .get("Mcp-Session-Id")
            .and_then(|s| s.to_str().ok())
            .ok_or(Error::Forbidden)?
            .to_owned();
        let connection = bridge
            .connections
            .lock()
            .map_err(|_| Error::Conflict)?
            .get(&session)
            .cloned()
            .ok_or(Error::McpSessionUnavailable)?;
        if connection
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Error::Conflict.into());
        }
        (session, connection)
    };
    let notification = body.get("id").is_none();
    let owner = bridge.clone();
    let sid = session.clone();
    // The blocking worker owns start/call/cleanup even if its HTTP future is dropped.
    let result = tokio::task::spawn_blocking(move || {
        let process = &connection.process;
        let result = (|| {
            if owner.closed.load(Ordering::Acquire) || cancel.load(Ordering::Acquire) {
                return Err(cowork_toolbox::Error::Conflict);
            }
            if initialize {
                toolbox.start_stdio_with_options(
                    process,
                    &id,
                    &owner.program,
                    owner.arguments.clone(),
                    cowork_toolbox::StdioOptions {
                        cwd: owner.cwd.as_deref(),
                        environment: &owner.environment,
                        native_cwd: owner.native_only,
                    },
                    &cancel,
                )?;
            }
            if owner.closed.load(Ordering::Acquire) || cancel.load(Ordering::Acquire) {
                return Err(cowork_toolbox::Error::Conflict);
            }
            toolbox.stdio_rpc(process, body, &cancel)
        })();
        *connection
            .last_used
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Instant::now();
        connection.busy.store(false, Ordering::Release);
        if result.is_err() || cancel.load(Ordering::Acquire) || owner.closed.load(Ordering::Acquire)
        {
            owner
                .connections
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&sid);
            let _ = toolbox.stop_persistent(process);
        }
        result
    })
    .await
    .map_err(|_| Error::UncertainEffect)?
    .map_err(|_| Error::McpSessionUnavailable)?;
    let mut response = if notification {
        StatusCode::ACCEPTED.into_response()
    } else {
        Json(result).into_response()
    };
    response.headers_mut().insert(
        "Mcp-Session-Id",
        HeaderValue::from_str(&session).map_err(|_| Error::Forbidden)?,
    );
    Ok(response)
}
async fn close(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Extension(toolbox): Extension<Arc<Toolbox>>,
    Extension(bridges): Extension<Arc<StdioBridges>>,
) -> std::result::Result<StatusCode, ApiFailure> {
    let bridge = authenticated(&state, &bridges, &toolbox, &id, &headers).await?;
    let session = headers
        .get("Mcp-Session-Id")
        .and_then(|s| s.to_str().ok())
        .ok_or(Error::Forbidden)?;
    let process = bridge
        .connections
        .lock()
        .map_err(|_| Error::Conflict)?
        .remove(session)
        .ok_or(Error::NotFound)?;
    tokio::task::spawn_blocking(move || toolbox.stop_persistent(&process.process))
        .await
        .map_err(|_| Error::UncertainEffect)?
        .map_err(|_| Error::UncertainEffect)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn factories(
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Answer<Vec<NativeMcpFactory>> {
    Ok(Json(
        state
            .runtime
            .store
            .native_mcp_factories(&project)
            .await?
            .into_iter()
            .filter(|factory| factory.native_only)
            .collect(),
    ))
}
pub(super) async fn remove_connections(
    bridges: &StdioBridges,
    toolbox: &Arc<Toolbox>,
    id: &str,
) -> std::result::Result<(), ApiFailure> {
    let bridge = bridges
        .entries
        .lock()
        .map_err(|_| Error::Conflict)?
        .remove(id);
    let processes = bridge
        .map(|b| {
            b.closed.store(true, Ordering::Release);
            let mut connections = b.connections.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *connections)
                .into_values()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let toolbox = toolbox.clone();
    tokio::task::spawn_blocking(move || {
        let mut error = None;
        for p in processes {
            if let Err(e) = toolbox.stop_persistent(&p.process) {
                error = Some(e);
            }
        }
        error.map_or(Ok(()), Err)
    })
    .await
    .map_err(|_| Error::UncertainEffect)?
    .map_err(|_| Error::UncertainEffect)?;
    Ok(())
}
async fn disconnect(
    State(state): State<AppState>,
    Path((project, server)): Path<(String, String)>,
    Extension(bridges): Extension<Arc<StdioBridges>>,
    Extension(toolbox): Extension<Arc<Toolbox>>,
    body: Input<RevisionRequest>,
) -> Answer<Value> {
    disconnect_inner(
        state,
        project,
        server,
        bridges,
        toolbox,
        input(body)?.expected_revision,
        true,
    )
    .await
}
async fn disconnect_inner(
    state: AppState,
    project: String,
    server: String,
    bridges: Arc<StdioBridges>,
    toolbox: Arc<Toolbox>,
    expected: i64,
    native: bool,
) -> Answer<Value> {
    if state.runtime.store.is_connector_server(&server).await? {
        return Err(Error::Forbidden.into());
    }
    let mut factory = state
        .runtime
        .store
        .native_mcp_factory(&server)
        .await?
        .ok_or(Error::NotFound)?;
    if factory.project_id != project || factory.native_only != native {
        return Err(Error::Conflict.into());
    }
    if !factory.enabled && expected.checked_add(1) == Some(factory.revision) {
        remove_connections(&bridges, &toolbox, &server).await?;
        return Ok(Json(
            json!({"disconnected":true,"revision":factory.revision}),
        ));
    }
    if factory.revision != expected {
        return Err(Error::Conflict.into());
    }
    factory.enabled = false;
    factory.revision = expected.checked_add(1).ok_or(Error::Conflict)?;
    state
        .runtime
        .store
        .put_native_mcp_factory(&factory, expected)
        .await?;
    remove_connections(&bridges, &toolbox, &server).await?;
    Ok(Json(
        json!({"disconnected":true,"revision":factory.revision}),
    ))
}

#[cfg(test)]
mod owner_connection_tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    struct Keys(cowork_secrets::KeyId);
    impl cowork_secrets::KeyStore for Keys {
        fn load(
            &self,
            id: &cowork_secrets::KeyId,
        ) -> cowork_secrets::Result<zeroize::Zeroizing<[u8; 32]>> {
            if id != &self.0 {
                return Err(cowork_secrets::Error::Missing);
            }
            Ok(zeroize::Zeroizing::new([7; 32]))
        }
    }
    async fn fixture(native: bool) -> (Router, cowork_storage::Store, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = cowork_storage::Store::sqlite(&dir.path().join("db"))
            .await
            .unwrap();
        let project = store
            .create_project(CreateProject { name: "MCP".into() })
            .await
            .unwrap();
        let key = cowork_secrets::KeyId {
            instance: uuid::Uuid::new_v4(),
            key: uuid::Uuid::new_v4(),
        };
        let runtime = Runtime::new(store.clone(), vec![], false)
            .unwrap()
            .with_personal_provider_keys(Arc::new(Keys(key)), key);
        let state = AppState {
            runtime,
            token: Arc::new("owner-fixture".into()),
            native_token: native.then(|| Arc::new("n".repeat(64))),
            origins: Arc::new(server_config::origins(43821, None).unwrap()),
        };
        let toolbox = Toolbox::open(cowork_toolbox::Config {
            root: dir.path().join("tools"),
            programs: BTreeMap::new(),
            environment: BTreeMap::new(),
            github_token: None,
        })
        .unwrap();
        let app = owner_router(toolbox, Arc::new(StdioBridges::default()))
            .layer(middleware::from_fn_with_state(state.clone(), authorize))
            .with_state(state);
        (app, store, project.id, dir)
    }
    async fn call(
        app: &Router,
        path: &str,
        body: Option<Value>,
        token: &str,
        origin: &str,
    ) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .method(if body.is_some() { "POST" } else { "GET" })
                    .header("authorization", format!("Bearer {token}"))
                    .header("origin", origin)
                    .header("content-type", "application/json")
                    .body(body.map_or(Body::empty(), |value| Body::from(value.to_string())))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    #[tokio::test]
    async fn owner_http_secret_endpoints_reject_actor_tokens_and_wrong_origins_then_recover_original_revision()
     {
        let (app, store, project, _dir) = fixture(false).await;
        let server = id();
        let secret = "synthetic-http-mcp-key";
        let body = json!({"project_id":project,"server_id":server,"endpoint":"https://tools.example/mcp","bearer":secret,"expected_revision":0});
        assert_eq!(
            call(
                &app,
                "/mcp/credential",
                Some(body.clone()),
                "actor-session",
                "http://127.0.0.1:43821"
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(
                &app,
                "/mcp/credential",
                Some(body.clone()),
                "owner-fixture",
                "https://hostile.invalid"
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert!(
            store
                .mcp_credential(&project, &format!("server-mcp:{server}"))
                .await
                .is_err()
        );
        let (status, saved) = call(
            &app,
            "/mcp/credential",
            Some(body.clone()),
            "owner-fixture",
            "http://127.0.0.1:43821",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!saved.to_string().contains(secret));
        assert_eq!(saved["credential_revision"], 1);
        assert_eq!(
            call(
                &app,
                "/mcp/credential",
                Some(body),
                "owner-fixture",
                "http://127.0.0.1:43821"
            )
            .await
            .1,
            saved
        );
        let metadata = call(
            &app,
            &format!("/mcp/credential/{project}/{server}"),
            None,
            "owner-fixture",
            "http://127.0.0.1:43821",
        )
        .await
        .1;
        assert!(!metadata.to_string().contains(secret));
        assert!(
            !serde_json::to_string(
                &store
                    .mcp_credential(&project, &format!("server-mcp:{server}"))
                    .await
                    .unwrap()
            )
            .unwrap()
            .contains(secret)
        );
    }
    #[tokio::test]
    async fn browser_owner_routes_cannot_bypass_native_stdio_confirmation() {
        let (app, _store, project, _dir) = fixture(true).await;
        let capabilities = call(
            &app,
            "/mcp/capabilities",
            None,
            "owner-fixture",
            "http://127.0.0.1:43821",
        )
        .await
        .1;
        assert_eq!(capabilities["stdio"], false);
        let body = json!({"project_id":project,"server_id":id(),"program":"python","arguments":[],"expected_revision":0});
        assert_eq!(
            call(
                &app,
                "/mcp/stdio",
                Some(body),
                "owner-fixture",
                "http://127.0.0.1:43821"
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
}
