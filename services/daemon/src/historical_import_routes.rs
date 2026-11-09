//! M9 product quarantine. Native owner and tenant session routes are separate.
use super::*;
use axum::{
    body::Bytes,
    http::{HeaderMap, Method},
};
use cowork_identity::{IdentityStore, Principal};
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{Store, historical_imports::product::*};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone)]
pub struct HistoricalImportServices {
    keys: Arc<dyn KeyStore>,
    key_id: KeyId,
    private_staging: Arc<std::path::PathBuf>,
    _private_guard: Arc<dyn Send + Sync>,
    objects: Arc<crate::s3_store::S3Store>,
    native_files: Option<Arc<cowork_broker::FileBroker>>,
    observers: Arc<tokio::sync::Mutex<ObserverRegistry>>,
    slots: Arc<tokio::sync::Semaphore>,
    body_slots: Arc<tokio::sync::Semaphore>,
}
#[derive(Default)]
struct ObserverRegistry {
    closed: bool,
    jobs: Vec<tokio::task::JoinHandle<()>>,
}
impl HistoricalImportServices {
    /// Caller retains the actually validated private startup-directory handle.
    pub fn from_startup(
        keys: Arc<dyn KeyStore>,
        key_id: KeyId,
        private_staging: std::path::PathBuf,
        private_guard: Arc<dyn Send + Sync>,
        objects: Arc<crate::s3_store::S3Store>,
        native_files: Option<Arc<cowork_broker::FileBroker>>,
    ) -> Self {
        Self {
            keys,
            key_id,
            private_staging: Arc::new(private_staging),
            _private_guard: private_guard,
            objects,
            native_files,
            observers: Arc::new(tokio::sync::Mutex::new(ObserverRegistry::default())),
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
            body_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }
    pub async fn shutdown(&self) {
        let jobs = {
            let mut registry = self.observers.lock().await;
            registry.closed = true;
            self.slots.close();
            self.body_slots.close();
            std::mem::take(&mut registry.jobs)
        };
        for job in jobs {
            let _ = job.await;
        }
    }
}
#[derive(Clone)]
struct ImportState {
    store: Store,
    identity: IdentityStore,
    services: HistoricalImportServices,
    native: bool,
    runtime: Option<Runtime>,
}
enum Access {
    Native(cowork_runtime::historical_imports::NativeHistoricalServices),
    Organization {
        actor: Principal,
        organization: Uuid,
        project: Uuid,
    },
}
impl Access {
    fn authority(&self) -> HistoricalAuthority<'_> {
        match self {
            Self::Native(a) => HistoricalAuthority::PersonalNative(&a.authority),
            Self::Organization {
                actor,
                organization,
                project,
            } => HistoricalAuthority::Organization {
                actor,
                organization_id: *organization,
                admission_project_id: *project,
            },
        }
    }
}
impl Access {
    fn key_id(&self, services: &HistoricalImportServices) -> KeyId {
        match self {
            Self::Native(n) => n.key_id,
            _ => services.key_id,
        }
    }
    fn keys<'a>(&'a self, services: &'a HistoricalImportServices) -> &'a dyn KeyStore {
        match self {
            Self::Native(n) => n.keys.as_ref(),
            _ => services.keys.as_ref(),
        }
    }
}
type Parameters = HashMap<String, String>;
fn selected(parameters: &Parameters, name: &str) -> Result<Uuid, ApiFailure> {
    Uuid::parse_str(parameters.get(name).ok_or(Error::NotFound)?)
        .map_err(|_| Error::Validation("Invalid historical selection".into()).into())
}
async fn access(
    state: &ImportState,
    headers: &HeaderMap,
    parameters: &Parameters,
) -> Result<Access, ApiFailure> {
    if state.native {
        return Ok(Access::Native(
            state
                .runtime
                .as_ref()
                .ok_or(Error::Forbidden)?
                .native_historical_keys(key_selection(parameters)?)
                .await?,
        ));
    }
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values
        .next()
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .filter(|v| !v.is_empty() && v.len() <= 4096)
        .ok_or(Error::Forbidden)?;
    if values.next().is_some() {
        return Err(Error::Forbidden.into());
    }
    let actor = state
        .identity
        .authenticate_session(value)
        .await
        .map_err(|_| ApiFailure(Error::Forbidden))?;
    Ok(Access::Organization {
        actor,
        organization: selected(parameters, "team")?,
        project: selected(parameters, "project")?,
    })
}
fn key_selection(p: &Parameters) -> Result<HistoricalKeySelection, ApiFailure> {
    if p.contains_key("preview") {
        return Ok(HistoricalKeySelection::Preview {
            preview: selected(p, "preview")?,
        });
    }
    if p.contains_key("import") {
        let import = selected(p, "import")?;
        if p.contains_key("stream") {
            let (stream, index) = chunk(p)?;
            return Ok(HistoricalKeySelection::Chunk {
                import,
                stream,
                index,
            });
        }
        return Ok(HistoricalKeySelection::Import { import });
    }
    Ok(HistoricalKeySelection::Active)
}
/// Runs before Bytes/Json extraction. Public invalid sessions cannot allocate
/// file bodies, and even authorized body intake has a fixed service bound.
async fn body_admission(
    State(state): State<ImportState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiFailure> {
    use axum::extract::FromRequestParts;
    let (mut parts, body) = request.into_parts();
    if !state.native {
        let Path(parameters) = Path::<Parameters>::from_request_parts(&mut parts, &state)
            .await
            .map_err(|_| Error::Forbidden)?;
        let actor = access(&state, &parts.headers, &parameters).await?;
        state
            .store
            .historical_import_authority(
                actor.authority(),
                actor.key_id(&state.services),
                cowork_identity::ContentAction::Read,
            )
            .await?;
    }
    let _slot = if parts.method != Method::GET {
        Some(
            state
                .services
                .body_slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| Error::WorkspaceBusy)?,
        )
    } else {
        None
    };
    Ok(next.run(Request::from_parts(parts, body)).await)
}
fn routes(prefix: &str, state: ImportState) -> Router<AppState> {
    let native = state.native;
    let router = Router::new()
        .route(&format!("{prefix}/limit"), get(limit))
        .route(&format!("{prefix}/targets"), get(targets))
        .route(
            &format!("{prefix}/previews/{{preview}}/files/{{source}}/stage"),
            get(file_operation)
                .post(stage_file)
                .layer(DefaultBodyLimit::max(if native {
                    64 * 1024 * 1024
                } else {
                    32 * 1024 * 1024
                })),
        )
        .route(&format!("{prefix}/begin"), post(begin))
        .route(&format!("{prefix}/imports/{{import}}"), get(record))
        .route(&format!("{prefix}/imports/{{import}}/history"),get(history_index))
        .route(&format!("{prefix}/imports/{{import}}/history/index"),post(build_history_index))
        .route(&format!("{prefix}/imports/{{import}}/history/tables"),get(history_tables))
        .route(&format!("{prefix}/imports/{{import}}/history/tables/{{table}}/rows"),get(history_rows))
        .route(&format!("{prefix}/imports/{{import}}/history/tables/{{table}}/rows/{{position}}/cells/{{page}}"),get(history_cells))
        .route(&format!("{prefix}/imports/{{import}}/history/tables/{{table}}/rows/{{position}}/cells/{{column}}/bytes/{{page}}"),get(history_value))
        .route(
            &format!("{prefix}/imports/{{import}}/chunks/{{stream}}/{{index}}"),
            get(read_chunk)
                .post(write_chunk)
                .layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route(&format!("{prefix}/imports/{{import}}/verify"), post(verify))
        .route(&format!("{prefix}/imports/{{import}}/content/catalog"),get(catalog_summary))
        .route(&format!("{prefix}/imports/{{import}}/content/catalog/{{source}}/{{page}}"),get(catalog_metadata))
        .route(&format!("{prefix}/imports/{{import}}/preview/paged"),post(preview_paged))
        .route(&format!("{prefix}/previews/{{preview}}/summary"),get(preview_summary))
        .route(&format!("{prefix}/previews/{{preview}}/projects/{{source}}/{{page}}"),get(preview_project))
        .route(
            &format!("{prefix}/imports/{{import}}/content"),
            get(catalog),
        )
        .route(
            &format!("{prefix}/imports/{{import}}/content/{{source}}/{{page}}"),
            get(content_page),
        )
        .route(
            &format!("{prefix}/imports/{{import}}/preview"),
            post(preview),
        )
        .route(&format!("{prefix}/previews/{{preview}}"), get(read_preview))
        .route(
            &format!("{prefix}/previews/{{preview}}/activate"),
            post(activate),
        )
        .route(
            &format!("{prefix}/previews/{{preview}}/receipt"),
            get(receipt),
        );
    let router = router.route(
        &format!("{prefix}/previews/{{preview}}/files/{{source}}/close-unsent"),
        post(close_unsent),
    );
    let router = if native {
        router.route(&format!("{prefix}/limit"), post(configure_limit))
    } else {
        router
    };
    router
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .layer(middleware::from_fn(private_response))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            body_admission,
        ))
        .with_state(state)
}
/// Merge below /api/v2/native INSIDE existing owner bearer middleware.
pub(super) fn native_router(
    state: AppState,
    services: HistoricalImportServices,
) -> Router<AppState> {
    routes(
        "/historical-import",
        ImportState {
            store: state.runtime.store.clone(),
            identity: state.runtime.store.identity(),
            services,
            native: true,
            runtime: Some(state.runtime.clone()),
        },
    )
    .layer(middleware::from_fn_with_state(
        state,
        native_folders::authorize_native,
    ))
}
/// Independent Global Principal authentication; native installation bearer cannot stand in for it.
pub(super) fn organization_router(
    store: Store,
    identity: IdentityStore,
    services: HistoricalImportServices,
) -> Router<AppState> {
    routes(
        "/api/v2/historical-import/{team}/{project}",
        ImportState {
            store,
            identity,
            services,
            native: false,
            runtime: None,
        },
    )
}
async fn private_response(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
async fn limit(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Answer<HistoricalImportLimit> {
    let actor = access(&state, &headers, &p).await?;
    Ok(Json(
        state
            .store
            .historical_import_limit(actor.authority(), actor.key_id(&state.services))
            .await?,
    ))
}
async fn begin(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
    body: Input<BeginHistoricalImport>,
) -> Answer<HistoricalImportRecord> {
    let actor = access(&state, &headers, &p).await?;
    Ok(Json(
        state
            .store
            .begin_historical_import(
                actor.authority(),
                actor.key_id(&state.services),
                input(body)?,
            )
            .await?,
    ))
}
async fn record(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Answer<HistoricalImportRecord> {
    let actor = access(&state, &headers, &p).await?;
    Ok(Json(
        state
            .store
            .historical_import_record(
                actor.authority(),
                actor.key_id(&state.services),
                selected(&p, "import")?,
            )
            .await?,
    ))
}
fn chunk(p: &Parameters) -> Result<(HistoricalStream, u32), ApiFailure> {
    let s = p.get("stream").ok_or(Error::NotFound)?;
    let stream = HistoricalStream::all()
        .into_iter()
        .find(|k| k.locator() == s)
        .ok_or(Error::NotFound)?;
    let index = p
        .get("index")
        .ok_or(Error::NotFound)?
        .parse()
        .map_err(|_| ApiFailure(Error::Validation("Invalid historical chunk".into())))?;
    Ok((stream, index))
}
async fn write_chunk(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
    bytes: Bytes,
) -> Answer<HistoricalChunkReceipt> {
    let actor = access(&state, &headers, &p).await?;
    let (stream, index) = chunk(&p)?;
    Ok(Json(
        state
            .store
            .write_historical_chunk(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "import")?,
                stream,
                index,
                &bytes,
            )
            .await?,
    ))
}
fn binary(bytes: zeroize::Zeroizing<Vec<u8>>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=historical-content",
            ),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes.to_vec(),
    )
        .into_response()
}
async fn read_chunk(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Result<Response, ApiFailure> {
    let actor = access(&state, &headers, &p).await?;
    let (stream, index) = chunk(&p)?;
    let bytes = state
        .store
        .read_historical_chunk(
            actor.authority(),
            actor.keys(&state.services),
            actor.key_id(&state.services),
            selected(&p, "import")?,
            stream,
            index,
        )
        .await?;
    Ok(binary(bytes))
}
async fn verify(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Answer<HistoricalImportRecord> {
    let actor = access(&state, &headers, &p).await?;
    Ok(Json(
        state
            .store
            .verify_historical_import(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "import")?,
                state.services.private_staging.as_path(),
            )
            .await?,
    ))
}
#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    offset: u32,
    #[serde(default = "page_size")]
    limit: u32,
}
fn page_size() -> u32 {
    50
}
async fn catalog(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
    Query(page): Query<Page>,
) -> Answer<Vec<HistoricalContentCatalogEntry>> {
    let actor = access(&state, &headers, &p).await?;
    Ok(Json(
        state
            .store
            .historical_content_catalog(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "import")?,
                page.offset,
                page.limit,
            )
            .await?,
    ))
}
async fn content_page(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Result<Response, ApiFailure> {
    let actor = access(&state, &headers, &p).await?;
    let source = p
        .get("source")
        .ok_or(Error::NotFound)?
        .parse()
        .map_err(|_| ApiFailure(Error::NotFound))?;
    let page = p
        .get("page")
        .ok_or(Error::NotFound)?
        .parse()
        .map_err(|_| ApiFailure(Error::NotFound))?;
    Ok(binary(
        state
            .store
            .historical_content_page(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "import")?,
                source,
                page,
            )
            .await?,
    ))
}
async fn catalog_summary(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
    Query(page): Query<HistoryPage>,
) -> Answer<HistoricalContentCatalogPage> {
    let actor = access(&state, &headers, &p).await?;
    Ok(Json(
        state
            .store
            .historical_content_catalog_page(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "import")?,
                page.offset,
                page.limit,
            )
            .await?,
    ))
}
async fn catalog_metadata(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Result<Response, ApiFailure> {
    let actor = access(&state, &headers, &p).await?;
    Ok(binary(
        state
            .store
            .historical_catalog_metadata_page(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "import")?,
                history_number(&p, "source")?,
                history_number(&p, "page")?,
            )
            .await?,
    ))
}
async fn preview_paged(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
    body: Input<PreviewHistoricalImport>,
) -> Answer<HistoricalPreviewSummary> {
    let actor = access(&state, &headers, &p).await?;
    let value = state
        .store
        .preview_historical_import(
            actor.authority(),
            actor.keys(&state.services),
            actor.key_id(&state.services),
            selected(&p, "import")?,
            input(body)?,
        )
        .await?;
    Ok(Json(historical_preview_summary(&value)?))
}
async fn preview_summary(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Answer<HistoricalPreviewSummary> {
    let actor = access(&state, &headers, &p).await?;
    let value = state
        .store
        .historical_import_preview(
            actor.authority(),
            actor.keys(&state.services),
            actor.key_id(&state.services),
            selected(&p, "preview")?,
        )
        .await?;
    Ok(Json(historical_preview_summary(&value)?))
}
async fn preview_project(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Result<Response, ApiFailure> {
    let actor = access(&state, &headers, &p).await?;
    Ok(binary(
        state
            .store
            .historical_preview_project_page(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "preview")?,
                history_number(&p, "source")?,
                history_number(&p, "page")?,
            )
            .await?,
    ))
}
fn history_number<T: std::str::FromStr>(p: &Parameters, name: &str) -> Result<T, ApiFailure> {
    p.get(name)
        .ok_or(Error::NotFound)?
        .parse()
        .map_err(|_| Error::NotFound.into())
}
async fn history_access(
    state: &ImportState,
    headers: &HeaderMap,
    p: &Parameters,
    chunks: Vec<(String, i64)>,
) -> Result<Access, ApiFailure> {
    if !state.native {
        return access(state, headers, p).await;
    }
    Ok(Access::Native(
        state
            .runtime
            .as_ref()
            .ok_or(Error::Forbidden)?
            .native_historical_keys(HistoricalKeySelection::IndexChunks {
                import: selected(p, "import")?,
                chunks,
            })
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryPage {
    #[serde(default)]
    offset: u32,
    #[serde(default = "history_page_size")]
    limit: u32,
}
fn history_page_size() -> u32 {
    10
}
async fn history_index(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Answer<HistoricalHistoryIndex> {
    let actor = history_access(&state, &headers, &p, vec![("history:header".into(), 0)]).await?;
    Ok(Json(
        state
            .store
            .historical_history_index(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "import")?,
            )
            .await?,
    ))
}
async fn build_history_index(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
    body: Input<BuildHistoricalHistoryIndex>,
) -> Answer<HistoricalHistoryIndex> {
    let actor = access(&state, &headers, &p).await?;
    let input = input(body)?;
    let import = selected(&p, "import")?;
    let slot = state
        .services
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::WorkspaceBusy)?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let own = state.clone();
    {
        let mut registry = state.services.observers.lock().await;
        if registry.closed {
            return Err(Error::WorkspaceBusy.into());
        }
        registry.jobs.retain(|job| !job.is_finished());
        registry.jobs.push(tokio::spawn(async move {
            let _slot = slot;
            // A disconnected HTTP waiter cannot abandon an admitted index transaction.
            // Timeout rolls back derivation; it never resends an external operation.
            let result = tokio::time::timeout(
                Duration::from_secs(6 * 60 * 60),
                own.store.build_historical_history_index(
                    actor.authority(),
                    actor.keys(&own.services),
                    actor.key_id(&own.services),
                    import,
                    input,
                    &own.services.private_staging,
                ),
            )
            .await
            .map_err(|_| Error::WorkspaceBusy)
            .and_then(|v| v);
            let _ = sender.send(result);
        }));
    }
    Ok(Json(receiver.await.map_err(|_| Error::UncertainEffect)??))
}
async fn history_tables(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
    Query(page): Query<HistoryPage>,
) -> Answer<HistoricalHistoryTablesPage> {
    if page.limit == 0 || page.limit > 10 {
        return Err(Error::Validation("Invalid original history page".into()).into());
    }
    let mut chunks = vec![("history:header".into(), 0)];
    for i in page.offset..page.offset.checked_add(page.limit).ok_or(Error::NotFound)? {
        chunks.push((format!("history:table:{i}"), 0));
    }
    let actor = history_access(&state, &headers, &p, chunks).await?;
    Ok(Json(
        state
            .store
            .historical_history_tables(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "import")?,
                page.offset,
                page.limit,
            )
            .await?,
    ))
}
async fn history_rows(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
    Query(page): Query<HistoryPage>,
) -> Answer<HistoricalHistoryRowsPage> {
    if page.limit == 0 || page.limit > 25 {
        return Err(Error::Validation("Invalid original history page".into()).into());
    }
    let table: u32 = history_number(&p, "table")?;
    let mut chunks = vec![
        ("history:header".into(), 0),
        (format!("history:table:{table}"), 0),
    ];
    for i in page.offset..page.offset.checked_add(page.limit).ok_or(Error::NotFound)? {
        chunks.push((format!("history:row:{table}"), i64::from(i)));
    }
    let actor = history_access(&state, &headers, &p, chunks).await?;
    Ok(Json(
        state
            .store
            .historical_history_rows(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "import")?,
                table,
                u64::from(page.offset),
                page.limit,
            )
            .await?,
    ))
}
async fn history_cells(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Answer<HistoricalHistoryCellsPage> {
    let table: u32 = history_number(&p, "table")?;
    let position: u64 = history_number(&p, "position")?;
    let page: u32 = history_number(&p, "page")?;
    let row = position
        .checked_sub(1)
        .and_then(|n| i64::try_from(n).ok())
        .ok_or(Error::NotFound)?;
    let actor = history_access(
        &state,
        &headers,
        &p,
        vec![
            ("history:header".into(), 0),
            (format!("history:table:{table}"), 0),
            (format!("history:row:{table}"), row),
            (format!("history:cells:{table}:{position}"), i64::from(page)),
        ],
    )
    .await?;
    Ok(Json(
        state
            .store
            .historical_history_cells(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "import")?,
                table,
                position,
                page,
            )
            .await?,
    ))
}
async fn history_value(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Result<Response, ApiFailure> {
    let table: u32 = history_number(&p, "table")?;
    let position: u64 = history_number(&p, "position")?;
    let column: u32 = history_number(&p, "column")?;
    let page: u32 = history_number(&p, "page")?;
    let row = position
        .checked_sub(1)
        .and_then(|n| i64::try_from(n).ok())
        .ok_or(Error::NotFound)?;
    let actor = history_access(
        &state,
        &headers,
        &p,
        vec![
            ("history:header".into(), 0),
            (format!("history:table:{table}"), 0),
            (format!("history:row:{table}"), row),
            (
                format!("history:cells:{table}:{position}"),
                i64::from(column / 16),
            ),
            (
                format!("history:value:{table}:{position}:{column}"),
                i64::from(page),
            ),
        ],
    )
    .await?;
    Ok(binary(
        state
            .store
            .historical_history_value_page(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "import")?,
                table,
                position,
                column,
                page,
            )
            .await?,
    ))
}
async fn preview(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
    body: Input<PreviewHistoricalImport>,
) -> Answer<HistoricalImportPreview> {
    let actor = access(&state, &headers, &p).await?;
    Ok(Json(
        state
            .store
            .preview_historical_import(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "import")?,
                input(body)?,
            )
            .await?,
    ))
}
async fn read_preview(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Answer<HistoricalImportPreview> {
    let actor = access(&state, &headers, &p).await?;
    Ok(Json(
        state
            .store
            .historical_import_preview(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "preview")?,
            )
            .await?,
    ))
}
async fn activate(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
    body: Input<ActivateHistoricalImport>,
) -> Answer<HistoricalActivationReceipt> {
    let actor = access(&state, &headers, &p).await?;
    Ok(Json(
        state
            .store
            .activate_historical_import(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "preview")?,
                input(body)?,
            )
            .await?,
    ))
}
async fn receipt(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Answer<HistoricalActivationReceipt> {
    let actor = access(&state, &headers, &p).await?;
    Ok(Json(
        state
            .store
            .historical_activation_receipt(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "preview")?,
            )
            .await?,
    ))
}

async fn targets(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Answer<Vec<HistoricalImportTarget>> {
    let actor = access(&state, &headers, &p).await?;
    Ok(Json(
        state
            .store
            .historical_import_targets(actor.authority(), actor.key_id(&state.services))
            .await?,
    ))
}
async fn configure_limit(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
    body: Input<ConfigureHistoricalImportLimit>,
) -> Answer<HistoricalImportLimit> {
    if !state.native {
        return Err(Error::Forbidden.into());
    }
    let actor = access(&state, &headers, &p).await?;
    let input = input(body)?;
    // Current OS proof and exact native state are revalidated in the mutation TX.
    Ok(Json(
        state
            .store
            .configure_native_historical_import_limit(
                actor.authority(),
                actor.key_id(&state.services),
                input,
            )
            .await?,
    ))
}
async fn file_operation(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
) -> Answer<HistoricalFileOperation> {
    let actor = access(&state, &headers, &p).await?;
    Ok(Json(
        state
            .store
            .historical_file_operation(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "preview")?,
                p.get("source").ok_or(Error::NotFound)?,
            )
            .await?,
    ))
}
async fn close_unsent(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
    body: Input<CloseHistoricalUnsentFile>,
) -> Answer<HistoricalUnsentFileReceipt> {
    let actor = access(&state, &headers, &p).await?;
    Ok(Json(
        state
            .store
            .close_historical_unsent_file(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                selected(&p, "preview")?,
                p.get("source").ok_or(Error::NotFound)?,
                input(body)?,
            )
            .await?,
    ))
}
async fn stage_file(
    State(state): State<ImportState>,
    headers: HeaderMap,
    Path(p): Path<Parameters>,
    bytes: Bytes,
) -> Answer<HistoricalFileOperation> {
    let actor = access(&state, &headers, &p).await?;
    let source = p.get("source").ok_or(Error::NotFound)?.clone();
    let preview = selected(&p, "preview")?;
    let maximum = if state.native {
        cowork_contracts::MAX_DOWNLOAD_BYTES
    } else {
        cowork_storage::TEAM_BLOB_MAX_BYTES
    };
    if bytes.len() as u64 > maximum {
        return Err(
            Error::Validation("Historical file exceeds current storage limit".into()).into(),
        );
    }
    // Admission is bounded before any durable intent. No HTTP waiter owns the
    // original operation: cancellation after registration only loses its ACK.
    let slot = state
        .services
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::WorkspaceBusy)?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let observer_state = state.clone();
    {
        let mut registry = state.services.observers.lock().await;
        if registry.closed {
            return Err(Error::WorkspaceBusy.into());
        }
        registry.jobs.retain(|job| !job.is_finished());
        registry.jobs.push(tokio::spawn(async move {
            let _slot = slot;
            let result = stage_original(&observer_state, actor, preview, source, bytes).await;
            let _ = sender.send(result);
        }));
    }
    Ok(Json(receiver.await.map_err(|_| Error::UncertainEffect)??))
}
async fn stage_original(
    state: &ImportState,
    actor: Access,
    preview: Uuid,
    source: String,
    bytes: Bytes,
) -> cowork_domain::Result<HistoricalFileOperation> {
    let backend = if state.native {
        "native-file-broker-v1".to_owned()
    } else {
        state.services.objects.backend_id().to_owned()
    };
    let permit = state
        .store
        .prepare_historical_file(
            actor.authority(),
            actor.keys(&state.services),
            actor.key_id(&state.services),
            preview,
            HistoricalFileStageRequest {
                source_id: source,
                backend_id: backend,
                size: bytes.len() as u64,
                sha256: cowork_domain::hash(&bytes),
            },
        )
        .await?;
    let operation = permit.operation().clone();
    if operation.state != "received" {
        // This fence runs inside the registered observer, after provider waits
        // and immediately before IO. Replays only read the original object.
        let dispatch = state
            .store
            .admit_historical_file_dispatch(
                actor.authority(),
                actor.keys(&state.services),
                actor.key_id(&state.services),
                &operation,
            )
            .await?;
        historical_file_bridge::observe(
            &state.store,
            state.services.objects.clone(),
            state.services.native_files.clone(),
            &operation,
            dispatch.should_send(),
            bytes,
        )
        .await?;
    }
    // Capturing an original readback is independent of later target changes;
    // releasing it to the caller still requires current content authority.
    state
        .store
        .historical_staged_file(
            actor.authority(),
            actor.keys(&state.services),
            actor.key_id(&state.services),
            preview,
            &operation.source_id,
        )
        .await?;
    Ok(HistoricalFileOperation {
        state: "received".into(),
        ..operation
    })
}
#[path = "historical_file_bridge.rs"]
mod historical_file_bridge;
