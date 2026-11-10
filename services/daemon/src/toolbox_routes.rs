//! Explicit owner tool control; model execution must enter through Runtime admission.
use super::*;
use axum::Extension;
use cowork_toolbox::{Request as ToolRequest, Toolbox};

pub(crate) fn failure(error: cowork_toolbox::Error) -> ApiFailure {
    match error {
        cowork_toolbox::Error::NotFound => Error::NotFound.into(),
        cowork_toolbox::Error::Conflict => Error::Conflict.into(),
        cowork_toolbox::Error::Invalid(message) => Error::Validation(message).into(),
        _ => Error::UncertainEffect.into(),
    }
}
pub(super) fn router(service: Arc<Toolbox>) -> Router<AppState> {
    Router::new()
        .merge(crate::browser_routes::router(service.clone()))
        .route("/toolbox", get(availability))
        .route("/orders/{order}/toolbox/preview", post(preview))
        .route("/orders/{order}/toolbox/jobs", get(jobs).post(submit))
        .route("/orders/{order}/toolbox/jobs/{job}", get(status))
        .route("/orders/{order}/toolbox/jobs/{job}/cancel", post(cancel))
        .route(
            "/orders/{order}/toolbox/browser-sessions",
            post(browser_session),
        )
        .route(
            "/orders/{order}/toolbox/browser-sessions/{session}/close",
            post(browser_close),
        )
        .route("/orders/{order}/toolbox/files", get(download).put(upload))
        .route(
            "/orders/{order}/toolbox/document-versions",
            get(document_versions),
        )
        .route(
            "/orders/{order}/toolbox/document-versions/{version}",
            get(document_version),
        )
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024))
        .layer(Extension(service))
}
pub(super) fn native_router(state: AppState, service: Arc<Toolbox>) -> Router<AppState> {
    Router::new()
        .route("/toolbox/reload", post(reload))
        .layer(Extension(service))
        .layer(middleware::from_fn_with_state(
            state,
            native_folders::authorize_native,
        ))
}
async fn reload(
    Extension(service): Extension<Arc<Toolbox>>,
) -> Answer<cowork_toolbox::Availability> {
    let data = service.setup_data_dir().map_err(failure)?;
    let path = data.join("toolbox.json");
    for parent in path.ancestors() {
        let Ok(metadata) = std::fs::symlink_metadata(parent) else {
            continue;
        };
        if metadata.is_symlink() {
            return Err(Error::Forbidden.into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(Error::Forbidden.into());
            }
        }
    }
    let raw = std::fs::read(&path).map_err(|_| {
        ApiFailure(Error::Validation(
            "Tool setup configuration is unavailable".into(),
        ))
    })?;
    if raw.len() > 256 * 1024 {
        return Err(Error::Forbidden.into());
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Settings {
        programs: std::collections::BTreeMap<String, std::path::PathBuf>,
        #[serde(default)]
        github_token_file: Option<std::path::PathBuf>,
    }
    let settings: Settings = serde_json::from_slice(&raw)
        .map_err(|_| ApiFailure(Error::Validation("Invalid tool setup configuration".into())))?;
    let _ = settings.github_token_file; // Setup does not reload or expose credentials.
    Ok(Json(
        service
            .reload_programs(settings.programs)
            .map_err(failure)?,
    ))
}
async fn order(state: &AppState, id: &str) -> std::result::Result<(), ApiFailure> {
    state.runtime.store.order(id).await?;
    Ok(())
}
async fn availability(
    Extension(service): Extension<Arc<Toolbox>>,
) -> Json<cowork_toolbox::Availability> {
    Json(service.availability())
}
async fn preview(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path(id): Path<String>,
    body: Input<ToolRequest>,
) -> Answer<cowork_toolbox::Preview> {
    let _lease = service.workspace_lease(&id).map_err(failure)?;
    state.runtime.store.native_chat_available(&id).await?;
    Ok(Json(service.preview(&id, &input(body)?).map_err(failure)?))
}
async fn submit(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path(id): Path<String>,
    body: Input<ToolRequest>,
) -> Answer<cowork_toolbox::Job> {
    order(&state, &id).await?;
    let request = input(body)?;
    let runtime = tokio::runtime::Handle::current();
    let store = state.runtime.store.clone();
    let job = tokio::task::spawn_blocking(move || {
        service.submit_checked(&id, request, || {
            runtime
                .block_on(store.native_chat_available(&id))
                .map_err(|error| match error {
                    Error::NotFound => cowork_toolbox::Error::NotFound,
                    _ => cowork_toolbox::Error::Conflict,
                })
        })
    })
    .await
    .map_err(|_| ApiFailure(Error::UncertainEffect))?
    .map_err(failure)?;
    Ok(Json(job))
}
async fn jobs(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path(id): Path<String>,
) -> Answer<Vec<cowork_toolbox::Job>> {
    order(&state, &id).await?;
    Ok(Json(service.jobs(&id).map_err(failure)?))
}
async fn status(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path((id, job)): Path<(String, String)>,
) -> Answer<cowork_toolbox::Job> {
    order(&state, &id).await?;
    Ok(Json(service.status(&id, &job).map_err(failure)?))
}
async fn cancel(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path((id, job)): Path<(String, String)>,
) -> Answer<cowork_toolbox::Job> {
    order(&state, &id).await?;
    Ok(Json(service.cancel(&id, &job).map_err(failure)?))
}
async fn browser_session(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path(id): Path<String>,
) -> Answer<cowork_toolbox::BrowserSession> {
    let _lease = service.workspace_lease(&id).map_err(failure)?;
    state.runtime.store.native_chat_available(&id).await?;
    Ok(Json(service.create_browser_session(&id).map_err(failure)?))
}
async fn browser_close(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path((id, session)): Path<(String, String)>,
) -> Answer<serde_json::Value> {
    order(&state, &id).await?;
    service
        .close_browser_session(&id, &session)
        .map_err(failure)?;
    Ok(Json(serde_json::json!({"closed":true})))
}
#[derive(Deserialize)]
struct FileQuery {
    path: String,
    expected_sha256: Option<String>,
}
async fn document_versions(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path(id): Path<String>,
    Query(query): Query<FileQuery>,
) -> Answer<Vec<cowork_toolbox::DocumentVersion>> {
    order(&state, &id).await?;
    Ok(Json(
        service
            .document_versions(&id, &query.path)
            .map_err(failure)?,
    ))
}
async fn document_version(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path((id, version)): Path<(String, String)>,
) -> std::result::Result<Response, ApiFailure> {
    order(&state, &id).await?;
    let (version, bytes) = service
        .read_document_version(&id, &version)
        .map_err(failure)?;
    Ok(binary_response(cowork_broker::BinaryFile {
        metadata: DownloadMetadata {
            file_name: version.file_name,
            size: version.size,
            sha256: version.sha256,
        },
        bytes,
    }))
}
async fn download(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path(id): Path<String>,
    Query(query): Query<FileQuery>,
) -> std::result::Result<Response, ApiFailure> {
    order(&state, &id).await?;
    let bytes = service
        .read_output_file(&id, &query.path)
        .map_err(failure)?;
    use sha2::{Digest, Sha256};
    let sha256 = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if query
        .expected_sha256
        .as_ref()
        .is_some_and(|expected| expected != &sha256)
    {
        return Err(Error::FileConflict.into());
    }
    let file_name = query
        .path
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .ok_or(Error::Forbidden)?
        .to_owned();
    Ok(binary_response(cowork_broker::BinaryFile {
        metadata: DownloadMetadata {
            file_name,
            size: bytes.len() as u64,
            sha256,
        },
        bytes,
    }))
}
async fn upload(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    Path(id): Path<String>,
    Query(query): Query<FileQuery>,
    body: axum::body::Bytes,
) -> Answer<serde_json::Value> {
    let lease = service.workspace_lease(&id).map_err(failure)?;
    state.runtime.store.native_chat_available(&id).await?;
    service
        .write_input_file_with_lease(&id, &query.path, &body, lease)
        .map_err(failure)?;
    Ok(Json(
        serde_json::json!({"path":query.path,"bytes":body.len()}),
    ))
}

#[cfg(test)]
mod owner_connection_tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use tower::ServiceExt;
    #[tokio::test]
    async fn deleted_chat_rejects_new_host_effects_but_retains_exact_job_receipts_and_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let store = cowork_storage::Store::sqlite(&dir.path().join("db"))
            .await
            .unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Retired workbench".into(),
            })
            .await
            .unwrap();
        let chat = store
            .create_order(CreateOrder {
                project_id: project.id,
                title: "Retire me".into(),
            })
            .await
            .unwrap();
        #[cfg(windows)]
        let program = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        #[cfg(unix)]
        let program = std::path::PathBuf::from("/bin/sh");
        let tools = dir.path().join("tools");
        let service = Toolbox::open(cowork_toolbox::Config {
            root: tools.clone(),
            programs: BTreeMap::from([("fixture".into(), program)]),
            environment: BTreeMap::new(),
            github_token: None,
        })
        .unwrap();
        service
            .write_input_file(&chat.id, "report.md", b"Preserved export")
            .unwrap();
        let mut original = cowork_toolbox::Request {
            idempotency_key: "original".into(),
            action: cowork_toolbox::Action::Terminal {
                program: "fixture".into(),
                arguments: vec![],
                cwd: "".into(),
            },
            timeout_seconds: 5,
            output_bytes: 1024,
            approval_sha256: None,
        };
        original.approval_sha256 =
            Some(service.preview(&chat.id, &original).unwrap().request_sha256);
        let receipt = cowork_toolbox::Job {
            id: cowork_storage::id(),
            order_id: chat.id.clone(),
            idempotency_key: original.idempotency_key.clone(),
            request_sha256: original.approval_sha256.clone().unwrap(),
            state: "completed".into(),
            outcome: None,
            error: None,
        };
        std::fs::write(
            tools.join("jobs").join(format!("{}.json", receipt.id)),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        store
            .delete_native_chat(
                &chat.id,
                &DeleteLibraryEntry {
                    expected_revision: chat.revision,
                    idempotency_key: "delete".into(),
                },
            )
            .await
            .unwrap();
        let state = AppState {
            runtime: Runtime::new(store, vec![], false).unwrap(),
            token: Arc::new("owner".into()),
            native_token: None,
            origins: Arc::new(server_config::origins(43821, None).unwrap()),
        };
        let app = router(service.clone())
            .layer(middleware::from_fn_with_state(state.clone(), authorize))
            .with_state(state);
        let request = |method: &str, path: &str, body: Vec<u8>| {
            Request::builder()
                .method(method)
                .uri(format!("/orders/{}/toolbox/{path}", chat.id))
                .header("authorization", "Bearer owner")
                .header("origin", "http://127.0.0.1:43821")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "POST",
                    "jobs",
                    serde_json::to_vec(&original).unwrap()
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK,
            "lost ACK returns original receipt without a new process"
        );
        let mut fresh = original;
        fresh.idempotency_key = "new-operation".into();
        fresh.approval_sha256 = Some(service.preview(&chat.id, &fresh).unwrap().request_sha256);
        assert_eq!(
            app.clone()
                .oneshot(request("POST", "jobs", serde_json::to_vec(&fresh).unwrap()))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            app.clone()
                .oneshot(request("POST", "browser-sessions", vec![]))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "PUT",
                    "files?path=new.md",
                    b"Forbidden new content".to_vec()
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            app.oneshot(request("GET", "files?path=report.md", vec![]))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(service.jobs(&chat.id).unwrap().len(), 1);
        assert!(!service.workspace(&chat.id).unwrap().join("new.md").exists());
    }
    #[tokio::test]
    async fn tool_file_downloads_bind_exact_bytes_name_size_and_optional_expected_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = cowork_storage::Store::sqlite(&dir.path().join("db"))
            .await
            .unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Documents".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id,
                title: "Document preview".into(),
            })
            .await
            .unwrap();
        let state = AppState {
            runtime: Runtime::new(store, vec![], false).unwrap(),
            token: Arc::new("owner".into()),
            native_token: None,
            origins: Arc::new(server_config::origins(43821, None).unwrap()),
        };
        let service = Toolbox::open(cowork_toolbox::Config {
            root: dir.path().join("tools"),
            programs: BTreeMap::new(),
            environment: BTreeMap::new(),
            github_token: None,
        })
        .unwrap();
        let app = router(service.clone())
            .layer(middleware::from_fn_with_state(state.clone(), authorize))
            .with_state(state);
        let path = format!("/orders/{}/toolbox/files?path=report.md", order.id);
        let bytes = "# Verified 🧠".as_bytes();
        let request = |method: &str, url: &str, token: &str, body: Vec<u8>| {
            Request::builder()
                .method(method)
                .uri(url)
                .header("authorization", format!("Bearer {token}"))
                .header("origin", "http://127.0.0.1:43821")
                .body(Body::from(body))
                .unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(request("PUT", &path, "actor-session", bytes.to_vec()))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(request("PUT", &path, "owner", bytes.to_vec()))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let response = app
            .clone()
            .oneshot(request("GET", &path, "owner", vec![]))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let digest = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/octet-stream"
        );
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            bytes.len().to_string()
        );
        assert_eq!(response.headers()["x-cowork-file-name"], "report%2Emd");
        assert_eq!(response.headers()["x-cowork-sha256"], digest);
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
            bytes
        );
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "GET",
                    &format!("{path}&expected_sha256={digest}"),
                    "owner",
                    vec![]
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        std::fs::write(
            service.workspace(&order.id).unwrap().join("report.md"),
            b"changed",
        )
        .unwrap();
        assert_eq!(
            app.oneshot(request(
                "GET",
                &format!("{path}&expected_sha256={digest}"),
                "owner",
                vec![]
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::CONFLICT
        );
    }
}
