#[cfg(windows)]
pub(crate) mod private_secret_file;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, Request, State, rejection::JsonRejection},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post, put},
};
use cowork_contracts::*;
use cowork_domain::Error;
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::id;
use cowork_workflow_contracts::{RunPreflight, RunRequest};
use futures_util::stream;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::VecDeque, convert::Infallible, sync::Arc, time::Duration};
use tower_http::{cors::CorsLayer, services::ServeDir};
mod actor_model_registry_routes;
mod android_auth_management_routes;
mod android_compat_routes;
mod android_device_channel;
pub mod android_device_services;
mod audit_routes;
mod automation_routes;
mod browser_routes;
mod codex_account_routes;
mod connector_routes;
mod device_hub_transport;
mod device_setup_routes;
mod environment_routes;
mod execution_input_routes;
mod execution_plan_routes;
mod execution_plan_v2_routes;
mod external_memory_routes;
pub mod historical_import_routes;
mod knowledge_access_routes;
mod knowledge_import_routes;
mod knowledge_topic_routes;
mod library_config_routes;
pub mod maintenance;
mod managed_process_routes;
mod managed_result_routes;
mod mcp_oauth_routes;
mod migration_import_routes;
mod model_context_routes;
mod native_audit;
mod native_auth_transport_routes;
mod native_folders;
pub mod native_lifecycle;
mod native_mcp;
mod native_secrets;
mod offline_mcp_routes;
mod offline_routes;
mod oidc_management_routes;
pub mod owner_terminal;
mod personal_money_routes;
mod personal_provider_routes;
mod productivity_routes;
mod provider_settings_routes;
pub mod s3_store;
pub mod server_config;
mod team_blob_routes;
mod team_routes;
mod team_schedule_routes;
mod team_tool_file_routes;
pub mod team_toolbox;
mod tenant_routes;
mod toolbox_routes;
mod workflow_journal_routes;
mod workflow_routes;
mod workspace_management_routes;
mod workspace_transfer_routes;

#[derive(Clone)]
pub struct AppState {
    pub runtime: Runtime,
    pub token: Arc<String>,
    /// Never included in connection_info, browser storage or general API responses.
    pub native_token: Option<Arc<String>>,
    pub origins: Arc<Vec<String>>,
}
pub struct ApiFailure(Error);
impl From<Error> for ApiFailure {
    fn from(e: Error) -> Self {
        Self(e)
    }
}
impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            Error::Validation(_) | Error::BudgetConfiguration | Error::ContextLimit => {
                StatusCode::BAD_REQUEST
            }
            Error::NotFound | Error::SandboxSessionNotFound => StatusCode::NOT_FOUND,
            Error::Conflict
            | Error::ExecutionPlanRequired
            | Error::ExecutionPlanExpired
            | Error::ExecutionPlanChanged
            | Error::StaleLease
            | Error::BudgetExceeded
            | Error::WorkflowClockUnverified
            | Error::WorkflowClockRollback => StatusCode::CONFLICT,
            Error::WorkspaceBusy
            | Error::TransferBusy
            | Error::FileConflict
            | Error::McpSessionUnavailable
            | Error::NativeKeyUnavailable
            | Error::UncertainEffect => StatusCode::CONFLICT,
            Error::Forbidden | Error::WorkflowRevoked => StatusCode::FORBIDDEN,
            Error::Provider(_) => StatusCode::BAD_GATEWAY,
            Error::Storage(_) => StatusCode::SERVICE_UNAVAILABLE,
        };
        let retryable = matches!(self.0, Error::Storage(_));
        let correlation_id = id();
        tracing::warn!(%correlation_id,code=self.0.code(),"API operation rejected");
        (
            status,
            Json(ApiError {
                code: self.0.code().into(),
                message: self.0.to_string(),
                retryable,
                correlation_id,
            }),
        )
            .into_response()
    }
}
type Answer<T> = std::result::Result<Json<T>, ApiFailure>;
type Input<T> = std::result::Result<Json<T>, JsonRejection>;
fn input<T>(value: Input<T>) -> std::result::Result<T, ApiFailure> {
    value
        .map(|Json(v)| v)
        .map_err(|_| ApiFailure(Error::Validation("Invalid JSON request".into())))
}
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiError {
            code: "authentication_required".into(),
            message: "Connect using the private V2 daemon token".into(),
            retryable: false,
            correlation_id: id(),
        }),
    )
        .into_response()
}
async fn authorize(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if let Some(origin) = request.headers().get(header::ORIGIN)
        && !origin
            .to_str()
            .ok()
            .is_some_and(|o| state.origins.iter().any(|a| a == o))
    {
        return ApiFailure(Error::Forbidden).into_response();
    }
    let expected = format!("Bearer {}", state.token);
    let actual = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    // Fixed length, compare all bytes. Token is random with >=256 bits of entropy.
    let mut different = actual.len() ^ expected.len();
    for (a, b) in actual.as_bytes().iter().zip(expected.as_bytes()) {
        different |= (*a ^ *b) as usize;
    }
    if different != 0 {
        return unauthorized();
    }
    next.run(request).await
}
async fn response_headers(request: Request, next: Next) -> Response {
    let api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    if api
        && (response.status().is_client_error() || response.status().is_server_error())
        && !response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| h.starts_with("application/json"))
    {
        response = (
            response.status(),
            Json(ApiError {
                code: "invalid_request".into(),
                message: "The API request could not be processed".into(),
                retryable: false,
                correlation_id: id(),
            }),
        )
            .into_response();
    }
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    headers.insert("content-security-policy",HeaderValue::from_static(
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; frame-src 'self' blob:; connect-src 'self' https: wss: ws://127.0.0.1:* ws://localhost:* http://127.0.0.1:* http://localhost:*; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"));
    response
}
pub fn router(state: AppState, web: Option<std::path::PathBuf>) -> Router {
    router_with_identity(state, web, None)
}

/// Tenant sessions authorize only the separately scoped identity/tenant services.
/// The existing single-owner bearer does not become a tenant principal.
pub fn router_with_identity(
    state: AppState,
    web: Option<std::path::PathBuf>,
    identity: Option<cowork_identity::IdentityStore>,
) -> Router {
    router_with_services(state, web, identity, None)
}
pub struct ProductServices {
    pub terminals: Arc<owner_terminal::TerminalService>,
    pub maintenance: Arc<maintenance::MaintenanceService>,
    pub teams: cowork_storage::team_service::TeamService,
    pub toolbox: Arc<cowork_toolbox::Toolbox>,
    pub team_tools: Arc<team_toolbox::TeamToolbox>,
    pub objects: Arc<s3_store::S3Store>,
    pub keys: Arc<dyn cowork_secrets::KeyStore>,
    pub key_id: cowork_secrets::KeyId,
    pub offline_authority: Arc<cowork_storage::offline_execution::OfflineAuthority>,
    pub android_devices: Arc<android_device_services::AndroidDeviceServices>,
    pub historical_imports: Option<historical_import_routes::HistoricalImportServices>,
}
pub async fn schedule_tick(runtime: &Runtime) -> cowork_domain::Result<usize> {
    productivity_routes::schedule_tick(runtime).await
}
pub fn router_with_services(
    state: AppState,
    web: Option<std::path::PathBuf>,
    identity: Option<cowork_identity::IdentityStore>,
    services: Option<ProductServices>,
) -> Router {
    let bridges = Arc::new(native_mcp::StdioBridges::default());
    let audit_authority = services
        .as_ref()
        .and_then(|s| cowork_identity::audit::AuditAuthority::new(s.keys.clone(), s.key_id).ok());
    let native = native_folders::router(state.clone())
        .merge(native_secrets::router(state.clone()))
        .merge(mcp_oauth_routes::native_router(state.clone()))
        .merge(native_audit::router(state.clone(), audit_authority.clone()))
        .merge(native_auth_transport_routes::router(state.clone()));
    let native = if let Some(imports) = services.as_ref().and_then(|s| s.historical_imports.clone())
    {
        native.merge(historical_import_routes::native_router(
            state.clone(),
            imports,
        ))
    } else {
        native
    };
    let native = if let Some(services) = &services {
        native
            .merge(migration_import_routes::router(
                state.clone(),
                services.toolbox.clone(),
            ))
            .merge(toolbox_routes::native_router(
                state.clone(),
                services.toolbox.clone(),
            ))
            .merge(connector_routes::native_router(
                state.clone(),
                services.toolbox.clone(),
                bridges.clone(),
            ))
            .merge(native_mcp::native_router(
                state.clone(),
                services.toolbox.clone(),
                bridges.clone(),
            ))
    } else {
        native
    };
    let api = Router::new()
        .merge(mcp_oauth_routes::owner_router())
        .merge(audit_routes::router(audit_authority.clone()))
        .merge(provider_settings_routes::router())
        .merge(knowledge_import_routes::router())
        .merge(knowledge_access_routes::router())
        .merge(knowledge_topic_routes::router())
        .merge(
            services
                .as_ref()
                .map(|s| connector_routes::owner_router(s.toolbox.clone(), bridges.clone()))
                .unwrap_or_default(),
        )
        .merge(
            services
                .as_ref()
                .map(|s| native_mcp::owner_router(s.toolbox.clone(), bridges.clone()))
                .unwrap_or_default(),
        )
        .merge(android_auth_management_routes::router())
        .merge(offline_routes::native_router(state.clone()))
        .merge(migration_import_routes::content_router())
        .merge(personal_money_routes::router())
        .merge(personal_provider_routes::router(state.clone()))
        .merge(library_config_routes::owner_router(
            state.runtime.store.clone(),
        ))
        .merge(external_memory_routes::owner_router(state.runtime.clone()))
        .merge(codex_account_routes::owner_router(state.runtime.clone()))
        .merge(workspace_management_routes::router())
        .merge(model_context_routes::router(state.clone()))
        .merge(actor_model_registry_routes::native_router(state.clone()))
        .merge(productivity_routes::router())
        .merge(automation_routes::router())
        .merge(environment_routes::router())
        .merge(execution_plan_routes::router())
        .merge(execution_plan_v2_routes::router())
        .merge(execution_input_routes::router())
        .merge(workspace_transfer_routes::router())
        .merge(managed_result_routes::router())
        .merge(managed_process_routes::router())
        .merge(workflow_routes::router())
        .merge(workflow_journal_routes::router())
        .nest("/native", native)
        .route("/health", get(health))
        .route("/providers", get(providers).post(create_provider))
        .route("/folders", get(folders))
        .route("/projects", get(projects).post(create_project))
        .route(
            "/projects/{project}/mcp-servers",
            get(mcp_servers).post(configure_mcp_server),
        )
        .route("/runs/{run}/mcp-attempts", get(mcp_attempts))
        .route(
            "/runs/{run}/mcp-attempts/{call}/reconcile",
            post(reconcile_mcp),
        )
        .route("/projects/{project}/mcp-discovery", post(discover_mcp))
        .route("/orders", get(orders).post(create_order))
        .route("/orders/{order}", get(order))
        .route("/orders/{order}/draft", put(save_draft))
        .route("/orders/{order}/messages", get(messages))
        .route("/orders/{order}/history", get(history))
        .route("/orders/{order}/workspace", get(workspace))
        .route("/orders/{order}/workspace/files", get(workspace_files))
        .route("/orders/{order}/workspace/text", get(workspace_text))
        .route("/orders/{order}/workspace/stat", get(workspace_stat))
        .route(
            "/orders/{order}/workspace/download",
            get(workspace_download),
        )
        .route(
            "/orders/{order}/runs/{run}/workspace/download",
            get(snapshot_download),
        )
        .route("/orders/{order}/export", get(export_order))
        .route("/orders/{order}/uploads", get(uploads).post(reserve_upload))
        .route(
            "/orders/{order}/uploads/{upload}/content",
            put(upload_content),
        )
        .route("/orders/{order}/branches", post(branch_order))
        .route("/activity", get(activity))
        .route("/search", get(search))
        .route("/orders/{order}/runs", get(runs))
        .route("/orders/{order}/artifacts", get(artifacts))
        .route("/preflight", post(preflight))
        .route("/runs", post(create_run))
        .route("/file-restorations", post(create_restore_run))
        .route("/file-imports", post(create_import_run))
        .route(
            "/orders/{order}/document-actions/prepare",
            post(prepare_document_action),
        )
        .route("/orders/{order}/document-drafts", put(save_document_draft))
        .route(
            "/orders/{order}/document-drafts/{draft}",
            get(document_draft),
        )
        .route(
            "/orders/{order}/document-drafts/{draft}/snapshots",
            get(document_draft_snapshots),
        )
        .route(
            "/orders/{order}/document-actions/by-key/{key}",
            get(document_preparation_by_key),
        )
        .route(
            "/orders/{order}/document-actions/{preparation}",
            get(document_preparation),
        )
        .route(
            "/orders/{order}/document-actions/{preparation}/commit",
            post(commit_document_action),
        )
        .route("/runs/{run}", get(detail))
        .route("/runs/{run}/events", get(events))
        .route("/runs/{run}/events/stream", get(event_stream))
        .route("/runs/{run}/cancel", post(cancel))
        .route("/runs/{run}/resume", post(resume))
        .route("/runs/{run}/approvals/{approval}", post(approval))
        .route("/runs/{run}/input", post(submit_input))
        .route("/runs/{run}/files/reconcile", post(reconcile_file))
        .route("/runs/{run}/file-changes/{call}/written", get(written_file))
        .route("/runs/{run}/previews/{sha}", get(run_preview))
        .route(
            "/runs/{run}/file-changes/{call}/previous",
            get(previous_file),
        )
        .fallback(|| async { ApiFailure(Error::NotFound) });
    let api = if let Some(services) = &services {
        api.merge(toolbox_routes::router(services.toolbox.clone()))
            .merge(knowledge_import_routes::extract_router(
                services.toolbox.clone(),
            ))
            .merge(owner_terminal::router(services.terminals.clone()))
            .merge(maintenance::router(services.maintenance.clone()))
            .merge(device_setup_routes::router(
                services.android_devices.clone(),
            ))
    } else {
        api
    };
    let api = api.layer(middleware::from_fn_with_state(state.clone(), authorize));
    let origins: Vec<HeaderValue> = state
        .origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();
    let mut app = Router::new()
        .route("/health", get(health))
        .merge(mcp_oauth_routes::public_router())
        .merge(automation_routes::public_router())
        .nest("/api/v2", api);
    if let Some(services) = &services {
        app = app.merge(native_mcp::bridge_router(services.toolbox.clone(), bridges));
    }
    if let Some(path) = web {
        app = app.fallback_service(ServeDir::new(path));
    }
    let store = state.runtime.store.clone();
    let import_routes = migration_import_routes::team_router(
        state.clone(),
        services.as_ref().map(|s| s.teams.clone()),
    )
    .with_state(state.clone());
    let historical_routes = if let Some((services, identity)) =
        services.as_ref().zip(identity.as_ref())
    {
        services.historical_imports.clone().map(|imports| {
            historical_import_routes::organization_router(store.clone(), identity.clone(), imports)
                .with_state(state.clone())
        })
    } else {
        None
    };
    let mut app = app.with_state(state.clone());
    if let Some(routes) = historical_routes {
        app = app.merge(routes);
    }
    if identity.is_some() {
        app = app.merge(codex_account_routes::actor_router(state.runtime.clone()));
        app = app.merge(library_config_routes::actor_router(store.clone()));
        app = app.merge(external_memory_routes::actor_router(state.runtime.clone()));
        app = app.merge(actor_model_registry_routes::actor_router(store.clone()));
    }
    if let Some(services) = services {
        app = app.merge(team_schedule_routes::router(services.teams.clone()));
        app = app.merge(offline_routes::team_router(
            services.teams.clone(),
            services.offline_authority,
        ));
        if let Some(identity) = &identity {
            app = app.merge(team_tool_file_routes::router(
                services.teams.clone(),
                identity.clone(),
                services.toolbox.clone(),
                services.team_tools,
            ));
        }
        app = app.merge(oidc_management_routes::router(
            store.clone(),
            services.keys.clone(),
            services.key_id,
        ));
        app = app.nest(
            "/api/v1",
            android_compat_routes::router(
                services.teams.clone(),
                services.objects.clone(),
                services.keys,
                services.key_id,
                services.android_devices,
            ),
        );
        if let Some(identity) = &identity {
            app = app.merge(team_blob_routes::router(
                store.clone(),
                identity.clone(),
                services.objects,
            ));
        }
        app = app.nest(
            "/api/v2/teams",
            team_routes::router(services.teams).merge(import_routes),
        );
    } else {
        app = app.route(
            "/api/v1/{*path}",
            axum::routing::any(|| async {
                (
                    StatusCode::NOT_IMPLEMENTED,
                    Json(ApiError {
                        code: "compatibility_not_available".into(),
                        message: "Compatibility requires configured team services".into(),
                        retryable: false,
                        correlation_id: id(),
                    }),
                )
            }),
        );
    }
    if let Some(identity) = identity {
        app = app
            .nest(
                "/api/v2/identity",
                cowork_identity::http::router_with_audit(identity.clone(), audit_authority),
            )
            .nest("/api/v2/tenant", tenant_routes::router(store, identity));
    }
    app.layer(DefaultBodyLimit::max(1_100_000))
        .layer(
            CorsLayer::new()
                .allow_origin(origins)
                .allow_headers([
                    header::AUTHORIZATION,
                    header::CONTENT_TYPE,
                    header::HeaderName::from_static("idempotency-key"),
                    header::HeaderName::from_static("last-event-id"),
                ])
                .expose_headers([
                    header::CONTENT_DISPOSITION,
                    header::HeaderName::from_static("x-cowork-file-name"),
                    header::HeaderName::from_static("x-cowork-sha256"),
                ])
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::PUT,
                    axum::http::Method::DELETE,
                ]),
        )
        .layer(middleware::from_fn(response_headers))
}
async fn health() -> Json<Health> {
    Json(Health {
        product: "LocalAI Cowork V2".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        schema_version: SCHEMA_VERSION,
        deployment: "single_owner_preview".into(),
        capabilities: vec![
            "personal_money_status".into(),
            "orders".into(),
            "durable_runs".into(),
            "exact_approvals".into(),
            "text_artifacts".into(),
            "native_workflows".into(),
            "workflow_library".into(),
            "project_skills_knowledge".into(),
            "timezone_schedules".into(),
            "workspace_archive".into(),
            "device_sync".into(),
            "bounded_native_offline_execution".into(),
            "mapped_v1_migration".into(),
            "android_device_compatibility".into(),
            "document_tools_v1".into(),
        ],
    })
}

async fn mcp_servers(
    State(s): State<AppState>,
    Path(project): Path<String>,
) -> Answer<Vec<McpServer>> {
    Ok(Json(s.runtime.store.mcp_servers(&project).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpDiscovery {
    endpoint: String,
    #[serde(default)]
    bearer: Option<String>,
    #[serde(default)]
    server_id: Option<String>,
    #[serde(default)]
    credential_revision: Option<i64>,
}
async fn discover_mcp(
    State(s): State<AppState>,
    Path(project): Path<String>,
    body: Input<McpDiscovery>,
) -> Answer<Value> {
    s.runtime
        .store
        .projects()
        .await?
        .into_iter()
        .find(|p| p.id == project)
        .ok_or(Error::NotFound)?;
    let request = input(body)?;
    let stored = if let (Some(server), Some(revision)) =
        (&request.server_id, request.credential_revision)
    {
        let reference = match s.runtime.store.mcp_server(&project, server).await {
            Ok(current) => {
                if current.configuration.endpoint != request.endpoint
                    || current.configuration.credential_revision != revision
                {
                    return Err(Error::Conflict.into());
                }
                current.configuration.credential_reference
            }
            Err(Error::NotFound) => Some(format!("owner-mcp:{server}")),
            Err(error) => return Err(error.into()),
        };
        match reference {
            Some(reference) => Some(
                s.runtime
                    .native_mcp_bearer(&project, server, &request.endpoint, &reference, revision)
                    .await?,
            ),
            None => None,
        }
    } else {
        None
    };
    let endpoint = cowork_mcp::Endpoint::new(
        &request.endpoint,
        stored
            .as_deref()
            .map(String::as_str)
            .or(request.bearer.as_deref()),
    )
    .map_err(|_| Error::Validation("Invalid MCP endpoint".into()))?;
    let discovered = tokio::time::timeout(Duration::from_secs(60), async {
        let session = s
            .runtime
            .connect_owner_mcp_catalog(
                request.server_id.as_deref().unwrap_or("discovery"),
                endpoint.url(),
                stored
                    .as_deref()
                    .map(String::as_str)
                    .or(request.bearer.as_deref()),
                cowork_mcp::Limits::default(),
                None,
                false,
            )
            .await?;
        let catalog = session.catalog().ok_or(Error::McpSessionUnavailable)?;
        let definitions: Vec<Value> = catalog
            .tools()
            .map(|(_, t)| t.definition().clone())
            .collect();
        Ok::<_, Error>(json!({"catalog":definitions,"catalog_sha256":catalog.sha256()}))
    })
    .await
    .map_err(|_| Error::McpSessionUnavailable)??;
    Ok(Json(discovered))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpReview {
    expected_revision: i64,
    request_sha256: String,
    review: Value,
}
async fn reconcile_mcp(
    State(s): State<AppState>,
    Path((run, call)): Path<(String, String)>,
    body: Input<McpReview>,
) -> Answer<McpAttempt> {
    let request = input(body)?;
    Ok(Json(
        s.runtime
            .store
            .reconcile_reviewed_mcp(
                &run,
                &call,
                request.expected_revision,
                &request.request_sha256,
                request.review,
            )
            .await?,
    ))
}
async fn configure_mcp_server(
    State(s): State<AppState>,
    Path(project): Path<String>,
    body: Input<ConfigureMcpServer>,
) -> Answer<McpServer> {
    let request = input(body)?;
    // This control-plane route is owner authenticated, never a model tool. It
    // validates/imports metadata only; storing a URL initiates no remote IO.
    Ok(Json(
        s.runtime
            .store
            .configure_mcp_server(
                &project,
                &request.server_id,
                request.base_revision,
                &request.idempotency_key,
                &request.configuration,
            )
            .await?
            .public_view(),
    ))
}
async fn mcp_attempts(
    State(s): State<AppState>,
    Path(run): Path<String>,
) -> Answer<Vec<McpAttempt>> {
    s.runtime.store.stored_run(&run).await?;
    Ok(Json(s.runtime.store.mcp_attempts(&run).await?))
}

/// A document preview PNG, only when this run's own tool results reference it.
async fn run_preview(
    State(s): State<AppState>,
    Path((run, sha)): Path<(String, String)>,
) -> Result<Response, ApiFailure> {
    let bytes = s.runtime.run_preview(&run, &sha).await?;
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    headers.insert(
        "x-cowork-sha256",
        HeaderValue::from_str(&sha).map_err(|_| ApiFailure(Error::NotFound))?,
    );
    Ok(response)
}

fn binary_response(file: cowork_broker::BinaryFile) -> Response {
    let name = percent_encoding::utf8_percent_encode(
        &file.metadata.file_name,
        percent_encoding::NON_ALPHANUMERIC,
    )
    .to_string();
    let mut response = file.bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    // Only ASCII percent-encoded bytes enter header values; never interpolate raw filenames.
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"download\"; filename*=UTF-8''{name}"
        ))
        .expect("encoded attachment name"),
    );
    headers.insert(
        "x-cowork-file-name",
        HeaderValue::from_str(&name).expect("encoded filename"),
    );
    headers.insert(
        "x-cowork-sha256",
        HeaderValue::from_str(&file.metadata.sha256).expect("SHA-256 hex"),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&file.metadata.size.to_string()).expect("decimal size"),
    );
    response
}

async fn workspace_download(
    State(s): State<AppState>,
    Path(order): Path<String>,
    Query(input): Query<DownloadFile>,
) -> Result<Response, ApiFailure> {
    Ok(binary_response(
        s.runtime.download_workspace(&order, input).await?,
    ))
}

async fn snapshot_download(
    State(s): State<AppState>,
    Path((order, run)): Path<(String, String)>,
    Query(input): Query<DownloadFile>,
) -> Result<Response, ApiFailure> {
    Ok(binary_response(
        s.runtime.download_snapshot(&order, &run, input).await?,
    ))
}
async fn providers(State(s): State<AppState>) -> Json<Vec<ProviderProfile>> {
    Json(s.runtime.profiles().await)
}
#[derive(Deserialize)]
struct UploadQuery {
    #[serde(default)]
    offset: u32,
}
async fn uploads(
    State(s): State<AppState>,
    Path(order): Path<String>,
    Query(q): Query<UploadQuery>,
) -> Answer<UploadPage> {
    Ok(Json(s.runtime.store.uploads(&order, q.offset, 30).await?))
}
async fn reserve_upload(
    State(s): State<AppState>,
    Path(order): Path<String>,
    body: Input<ReserveUpload>,
) -> Answer<FileUpload> {
    Ok(Json(s.runtime.reserve_upload(&order, &input(body)?).await?))
}
async fn upload_content(
    State(s): State<AppState>,
    Path((order, upload)): Path<(String, String)>,
    request: Request,
) -> Answer<FileUpload> {
    // Authentication/origin middleware runs before this handler; reserve bounded capacity before buffering.
    let permit = s.runtime.begin_upload_transfer()?;
    let record = s.runtime.store.upload(&order, &upload).await?;
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        != Some("application/octet-stream")
    {
        return Err(Error::Validation("Expected binary file content".into()).into());
    }
    if let Some(length) = request.headers().get(header::CONTENT_LENGTH)
        && length.to_str().ok().and_then(|v| v.parse::<u64>().ok()) != Some(record.file.size)
    {
        return Err(Error::Validation("Upload size does not match reservation".into()).into());
    }
    if record.file.size > MAX_DOWNLOAD_BYTES {
        return Err(Error::Forbidden.into());
    }
    let bytes = tokio::time::timeout(
        Duration::from_secs(60),
        axum::body::to_bytes(request.into_body(), record.file.size as usize),
    )
    .await
    .map_err(|_| {
        ApiFailure(Error::Validation(
            "Upload timed out; retry the same reservation".into(),
        ))
    })?
    .map_err(|_| ApiFailure(Error::Validation("Incomplete or oversized upload".into())))?;
    let content = bytes.to_vec();
    drop(bytes);
    Ok(Json(
        s.runtime
            .receive_upload_with_permit(&order, &upload, content, permit)
            .await?,
    ))
}
async fn create_import_run(
    State(s): State<AppState>,
    body: Input<CreateImportRun>,
) -> Answer<RunRecord> {
    let request = input(body)?;
    let existing = s.runtime.store.existing_import_request(&request).await?;
    existing
        .map(Json)
        .ok_or_else(|| ApiFailure(Error::ExecutionPlanRequired))
}
async fn prepare_document_action(
    State(s): State<AppState>,
    Path(order): Path<String>,
    body: Input<DocumentActionRequest>,
) -> Answer<DocumentPreparation> {
    Ok(Json(
        s.runtime
            .prepare_document_action(&order, &input(body)?)
            .await?,
    ))
}
async fn save_document_draft(
    State(s): State<AppState>,
    Path(order): Path<String>,
    body: Input<SaveDocumentDraft>,
) -> Answer<DocumentDraftSave> {
    Ok(Json(
        s.runtime
            .store
            .save_document_draft(&order, &input(body)?)
            .await?,
    ))
}
async fn document_draft(
    State(s): State<AppState>,
    Path((order, draft)): Path<(String, String)>,
) -> Answer<DocumentDraft> {
    Ok(Json(s.runtime.store.document_draft(&order, &draft).await?))
}
async fn document_draft_snapshots(
    State(s): State<AppState>,
    Path((order, draft)): Path<(String, String)>,
) -> Answer<Vec<DocumentDraftSnapshot>> {
    Ok(Json(
        s.runtime
            .store
            .document_draft_snapshots(&order, &draft)
            .await?,
    ))
}
async fn document_preparation(
    State(s): State<AppState>,
    Path((order, preparation)): Path<(String, String)>,
) -> Answer<DocumentPreparation> {
    Ok(Json(
        s.runtime.document_preparation(&order, &preparation).await?,
    ))
}
async fn document_preparation_by_key(
    State(s): State<AppState>,
    Path((order, key)): Path<(String, String)>,
) -> Answer<DocumentPreparation> {
    Ok(Json(
        s.runtime.document_preparation_by_key(&order, &key).await?,
    ))
}
async fn commit_document_action(
    State(s): State<AppState>,
    Path((order, preparation)): Path<(String, String)>,
) -> Answer<RunRecord> {
    Ok(Json(
        s.runtime
            .commit_document_action(&order, &preparation)
            .await?,
    ))
}
async fn create_provider(
    State(s): State<AppState>,
    body: Input<CreateProvider>,
) -> Answer<ProviderProfile> {
    let body = input(body)?;
    if matches!(body.kind, ProviderKind::Codex) {
        return Err(ApiFailure(Error::Validation(
            "Create Codex profiles through an isolated account".into(),
        )));
    }
    let p = ProviderProfile {
        generation: Default::default(),
        id: id(),
        name: body.name,
        kind: body.kind,
        endpoint: body.endpoint,
        model: body.model,
        revision: 1,
        context_tokens: body.context_tokens,
        credential_required: false,
        credential_ready: true,
        capabilities: body.capabilities,
    };
    cowork_runtime::provider::validate_profile(&p, s.runtime.allow_test)?;
    s.runtime.store.insert_provider(&p).await?;
    s.runtime.providers.write().await.push(ConfiguredProvider {
        profile: p.clone(),
        credential_env: None,
    });
    Ok(Json(p))
}
async fn projects(State(s): State<AppState>) -> Answer<Vec<Project>> {
    Ok(Json(s.runtime.store.projects().await?))
}
async fn create_project(State(s): State<AppState>, body: Input<CreateProject>) -> Answer<Project> {
    Ok(Json(s.runtime.store.create_project(input(body)?).await?))
}
#[derive(Deserialize)]
struct ProjectQuery {
    project_id: String,
}
async fn folders(
    State(s): State<AppState>,
    Query(q): Query<ProjectQuery>,
) -> Answer<Vec<FolderGrant>> {
    Ok(Json(
        s.runtime
            .files
            .as_ref()
            .map(|b| b.grants(&q.project_id))
            .transpose()?
            .unwrap_or_default(),
    ))
}
async fn reconcile_file(
    State(s): State<AppState>,
    Path(run): Path<String>,
    body: Input<ReconcileFile>,
) -> Answer<RunRecord> {
    Ok(Json(s.runtime.reconcile_file(&run, input(body)?).await?))
}
async fn previous_file(
    State(s): State<AppState>,
    Path((run, call)): Path<(String, String)>,
) -> Answer<TextFile> {
    Ok(Json(s.runtime.previous_file_version(&run, &call).await?))
}
async fn written_file(
    State(s): State<AppState>,
    Path((run, call)): Path<(String, String)>,
) -> Answer<TextFile> {
    Ok(Json(s.runtime.written_file_version(&run, &call).await?))
}
async fn orders(State(s): State<AppState>, Query(q): Query<ProjectQuery>) -> Answer<Vec<Order>> {
    Ok(Json(s.runtime.store.orders(&q.project_id).await?))
}
async fn create_order(State(s): State<AppState>, body: Input<CreateOrder>) -> Answer<Order> {
    Ok(Json(s.runtime.store.create_order(input(body)?).await?))
}
async fn order(State(s): State<AppState>, Path(order): Path<String>) -> Answer<Order> {
    Ok(Json(s.runtime.store.order(&order).await?))
}
async fn save_draft(
    State(s): State<AppState>,
    Path(order): Path<String>,
    body: Input<SaveDraft>,
) -> Answer<Order> {
    Ok(Json(
        s.runtime.store.save_draft(&order, input(body)?).await?,
    ))
}
async fn messages(State(s): State<AppState>, Path(order): Path<String>) -> Answer<Vec<Message>> {
    Ok(Json(s.runtime.store.messages(&order).await?))
}
#[derive(serde::Deserialize)]
struct WorkspacePath {
    path: String,
}
async fn workspace_files(
    State(s): State<AppState>,
    Path(order): Path<String>,
    Query(q): Query<WorkspacePath>,
) -> Answer<DirectoryListing> {
    Ok(Json(s.runtime.workspace_files(&order, q.path).await?))
}
async fn workspace_text(
    State(s): State<AppState>,
    Path(order): Path<String>,
    Query(q): Query<WorkspacePath>,
) -> Answer<TextFile> {
    Ok(Json(s.runtime.workspace_text(&order, q.path).await?))
}
async fn workspace(State(s): State<AppState>, Path(order): Path<String>) -> Answer<Workspace> {
    Ok(Json(s.runtime.workspace(&order).await?))
}
async fn workspace_stat(
    State(s): State<AppState>,
    Path(order): Path<String>,
    Query(q): Query<WorkspacePath>,
) -> Answer<FileStat> {
    Ok(Json(s.runtime.workspace_stat(&order, q.path).await?))
}
async fn history(State(s): State<AppState>, Path(order): Path<String>) -> Answer<OrderHistory> {
    Ok(Json(s.runtime.store.history(&order).await?))
}
async fn export_order(State(s): State<AppState>, Path(order): Path<String>) -> Answer<OrderExport> {
    Ok(Json(s.runtime.store.export_order(&order).await?))
}
async fn branch_order(
    State(s): State<AppState>,
    Path(order): Path<String>,
    body: Input<BranchOrder>,
) -> Answer<Order> {
    Ok(Json(
        s.runtime.store.branch_order(&order, input(body)?).await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivityQuery {
    #[serde(default)]
    attention_only: bool,
    #[serde(default)]
    offset: u32,
    #[serde(default = "default_page_size")]
    limit: u32,
}
fn default_page_size() -> u32 {
    30
}
async fn activity(
    State(s): State<AppState>,
    Query(q): Query<ActivityQuery>,
) -> Answer<ActivityPage> {
    Ok(Json(
        s.runtime
            .store
            .activity(q.attention_only, q.offset, q.limit)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchQuery {
    q: String,
    project_id: Option<String>,
    #[serde(default)]
    offset: u32,
    #[serde(default = "default_page_size")]
    limit: u32,
}
async fn search(State(s): State<AppState>, Query(q): Query<SearchQuery>) -> Answer<SearchPage> {
    Ok(Json(
        s.runtime
            .store
            .search(&q.q, q.project_id.as_deref(), q.offset, q.limit)
            .await?,
    ))
}
async fn runs(State(s): State<AppState>, Path(order): Path<String>) -> Answer<Vec<RunRecord>> {
    Ok(Json(s.runtime.store.runs(&order).await?))
}
async fn artifacts(State(s): State<AppState>, Path(order): Path<String>) -> Answer<Vec<Artifact>> {
    Ok(Json(s.runtime.store.artifacts(&order).await?))
}
async fn preflight(State(s): State<AppState>, body: Input<RunRequest>) -> Answer<RunPreflight> {
    Ok(Json(s.runtime.preflight_request(&input(body)?).await?))
}
async fn create_run(State(s): State<AppState>, body: Input<RunRequest>) -> Answer<RunRecord> {
    let request = input(body)?;
    // Historical ACK recovery remains available; an old client cannot create
    // fresh execution without reviewing the new plan contract.
    if let Some(prepared) = s.runtime.store.existing_execution_plan(&request).await?
        && let Some(receipt) = s.runtime.store.execution_plan_run(&prepared).await?
    {
        return Ok(Json(receipt));
    }
    let existing = match &request {
        RunRequest::Single(input) => s.runtime.store.existing_request(input).await?,
        RunRequest::Workflow(input) => {
            s.runtime
                .store
                .existing_product_workflow_request(input)
                .await?
        }
    };
    existing
        .map(Json)
        .ok_or_else(|| ApiFailure(Error::ExecutionPlanRequired))
}

async fn create_restore_run(
    State(s): State<AppState>,
    body: Input<CreateRestoreRun>,
) -> Answer<RunRecord> {
    let request = input(body)?;
    let existing = s.runtime.store.existing_restore_request(&request).await?;
    existing
        .map(Json)
        .ok_or_else(|| ApiFailure(Error::ExecutionPlanRequired))
}
async fn detail(State(s): State<AppState>, Path(run): Path<String>) -> Answer<RunDetail> {
    Ok(Json(s.runtime.store.detail(&run).await?))
}
#[derive(Deserialize, Default)]
struct EventsQuery {
    #[serde(default)]
    after: i64,
}
async fn events(
    State(s): State<AppState>,
    Path(run): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Answer<Vec<RunEvent>> {
    Ok(Json(s.runtime.store.events(&run, q.after).await?))
}
async fn event_stream(
    State(s): State<AppState>,
    Path(run): Path<String>,
    Query(q): Query<EventsQuery>,
) -> std::result::Result<
    Sse<impl futures_util::Stream<Item = std::result::Result<Event, Infallible>>>,
    ApiFailure,
> {
    s.runtime.store.stored_run(&run).await?;
    let stream = stream::unfold(
        (s.runtime.store, run, q.after, VecDeque::<RunEvent>::new()),
        |(store, run, mut after, mut pending)| async move {
            loop {
                if let Some(event) = pending.pop_front() {
                    after = event.sequence;
                    let output = Event::default()
                        .id(event.sequence.to_string())
                        .event("run_event")
                        .json_data(&event)
                        .unwrap_or_else(|_| Event::default().event("serialization_error"));
                    return Some((Ok(output), (store, run, after, pending)));
                }
                match store.events(&run, after).await {
                    Ok(events) => pending.extend(events),
                    Err(_) => return None,
                }
                if pending.is_empty() {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        },
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
async fn cancel(State(s): State<AppState>, Path(run): Path<String>) -> Answer<RunRecord> {
    Ok(Json(s.runtime.store.cancel(&run).await?))
}
async fn resume(State(s): State<AppState>, Path(run): Path<String>) -> Answer<RunRecord> {
    Ok(Json(s.runtime.store.resume(&run).await?))
}
async fn approval(
    State(s): State<AppState>,
    Path((run, approval)): Path<(String, String)>,
    body: Input<ResolveApproval>,
) -> Answer<RunRecord> {
    Ok(Json(
        s.runtime
            .store
            .resolve_approval(&run, &approval, input(body)?)
            .await?,
    ))
}
async fn submit_input(
    State(s): State<AppState>,
    Path(run): Path<String>,
    body: Input<SubmitInput>,
) -> Answer<RunRecord> {
    Ok(Json(
        s.runtime.store.submit_input(&run, input(body)?).await?,
    ))
}
