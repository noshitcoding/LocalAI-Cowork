use std::{
    collections::HashMap,
    env, fs,
    net::SocketAddr,
    process::Stdio,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use axum::{
    body::{Body, Bytes},
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        OriginalUri, Path, Query, State,
    },
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use cowork_contracts::{
    ensure_compatible, DesktopDimensions, SandboxDesktopSessionResult, SandboxDesktopSessionSpec,
    SandboxImage, SandboxNetwork, SandboxRunResult, SandboxRunSpec, SCHEMA_VERSION,
};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Mutex,
};
use tower_http::{catch_panic::CatchPanicLayer, trace::TraceLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod admission;
mod kubernetes;
mod tls;
use admission::{Admission, Resources};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    replay_cache: Arc<Mutex<HashMap<String, i64>>>,
    desktop_sessions: Arc<Mutex<HashMap<uuid::Uuid, SandboxDesktopSessionResult>>>,
    admission: Arc<Admission>,
    desktop_job_proofs: bool,
    kubernetes_desktop_proofs: bool,
    kubernetes: Option<Arc<kubernetes::Backend>>,
}

#[derive(Debug)]
struct Config {
    listen_addr: SocketAddr,
    signing_key: Vec<u8>,
    core_image: String,
    gui_image: String,
    crew_image: String,
    filtered_egress_network: Option<String>,
    filtered_egress_proxy: Option<String>,
    seccomp_profile: String,
    apparmor_profile: Option<String>,
    require_apparmor: bool,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: &'static str,
    message: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    if env::args().skip(1).collect::<Vec<_>>() == ["--kubernetes-journal-inspect"] {
        if env::var("COWORK_RUNNER_BACKEND").as_deref() != Ok("kubernetes") {
            bail!("native Kubernetes journal mode required");
        }
        let directory = std::path::PathBuf::from(env::var("COWORK_RUNNER_ADMISSION_DIRECTORY")?);
        println!("{}", kubernetes::Backend::inspect_journal(&directory)?);
        return Ok(());
    }
    if let Ok(instance) = env::var("COWORK_RUNNER_INSTANCE_ID") {
        uuid::Uuid::parse_str(&instance).context("invalid COWORK_RUNNER_INSTANCE_ID")?;
    }
    if let Ok(generation) = env::var("COWORK_RUNNER_STORAGE_GENERATION") {
        uuid::Uuid::parse_str(&generation).context("invalid COWORK_RUNNER_STORAGE_GENERATION")?;
        if env::var("COWORK_RUNNER_INSTANCE_ID").is_err() {
            bail!("storage generation requires an instance ID");
        }
    }
    let config = Arc::new(Config::from_env()?);
    let kubernetes = kubernetes::Backend::configured().await?;
    let tls = tls::configured()?;
    if kubernetes.is_some() && tls.is_none() {
        bail!("native Kubernetes runner requires serving TLS");
    }
    let desktop_job_proofs = env_flag("COWORK_RUNNER_DESKTOP_JOB_PROOFS", false)?;
    let kubernetes_desktop_proofs = env_flag("COWORK_RUNNER_KUBERNETES_DESKTOP_PROOFS", false)?;
    if kubernetes_desktop_proofs
        && (desktop_job_proofs || kubernetes.as_ref().is_none_or(|b| !b.desktop_enabled()))
    {
        bail!("native desktop proof opt-in requires the reviewed Kubernetes desktop profile");
    }
    if kubernetes.is_some() && desktop_job_proofs {
        bail!("Docker desktop proofs cannot be enabled on the Kubernetes backend");
    }
    if desktop_job_proofs && tls.is_none() {
        bail!("private desktop job proofs require native runner TLS");
    }
    let prefix = scoped_resource("", "").trim_end_matches('-').to_owned() + "-";
    let admission = Arc::new(Admission::new(
        env::var_os("COWORK_RUNNER_ADMISSION_DIRECTORY")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(".cowork-runner-admission")),
        prefix,
        admission::Limits::from_env()?,
    )?);
    if kubernetes.is_none() {
        docker(&["version", "--format", "{{.Server.Version}}"])
            .await
            .context("Docker daemon is unavailable")?;
        validate_sandbox_security(&config).await?;
    }
    if desktop_job_proofs {
        admission.initialize_desktop_identity().await?;
    }
    let recovered_desktops = if kubernetes.is_some() {
        HashMap::new()
    } else {
        recover_desktop_sessions().await?
    };
    if !recovered_desktops.is_empty() {
        tracing::info!(
            count = recovered_desktops.len(),
            "recovered persistent desktop sessions"
        );
    }
    let state = AppState {
        config: config.clone(),
        replay_cache: Arc::new(Mutex::new(HashMap::new())),
        desktop_sessions: Arc::new(Mutex::new(recovered_desktops)),
        admission,
        desktop_job_proofs,
        kubernetes_desktop_proofs,
        kubernetes,
    };
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/backend-profile", get(backend_profile))
        .route("/v1/kubernetes-scope", get(kubernetes_scope))
        .route("/v1/kubernetes-quiesce", post(kubernetes_quiesce))
        .route("/v1/kubernetes-activate", post(kubernetes_activate))
        .route(
            "/v1/jobs",
            post(run_job).layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route(
            "/v1/execution-stop",
            post(stop_execution).layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route(
            "/v2/jobs",
            post(run_job).layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route("/v2/runs/{run_id}/file", get(read_run_file))
        .route("/v2/runs/{run_id}/terminal", get(stream_terminal))
        .route("/v2/desktop-sessions", post(start_desktop_session))
        .route("/v1/runs/{run_id}/file", get(read_run_file))
        .route("/v1/runs/{run_id}/terminal", get(stream_terminal))
        .route("/v1/desktop-sessions", post(start_desktop_session))
        .route(
            "/v1/desktop-session-probe",
            post(probe_desktop_session).layer(axum::extract::DefaultBodyLimit::max(64 * 1024)),
        )
        .route(
            "/v1/desktop-sessions/{session_id}",
            axum::routing::delete(stop_desktop_session),
        )
        .route(
            "/v1/desktop-sessions/{session_id}/stream",
            get(stream_desktop_session),
        )
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(config.listen_addr).await?;
    tracing::info!(address = %config.listen_addr, "sandbox runner listening");
    if let Some(acceptor) = tls {
        axum::serve(tls::Listener::new(listener, acceptor), app).await?;
    } else {
        axum::serve(listener, app).await?;
    }
    Ok(())
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "backend": if state.kubernetes.is_some() { "kubernetes" } else { "docker" },
        "native_kubernetes_capabilities": if state.kubernetes.is_some() { Some(if state.kubernetes_desktop_proofs {
            vec!["sandbox.core", "sandbox.crew", "artifact.read", "execution.stop", "terminal", "desktop.linux", "browser.visible", "office.libreoffice"]
        } else { vec!["sandbox.core", "sandbox.crew", "artifact.read", "execution.stop", "terminal"] }) } else { None },
        "schema_version": SCHEMA_VERSION,
        "sandbox_capacity": state.admission.limits(),
        "chat_workspace_version": 1,
        "desktop_job_proofs_version": if state.kubernetes_desktop_proofs { Some(2) } else if state.desktop_job_proofs { Some(1) } else { None },
        "sandbox_security": {
            "seccomp": state.config.seccomp_profile,
            "apparmor": state.config.apparmor_profile,
            "apparmor_required": state.config.require_apparmor,
        }
    }))
}

async fn backend_profile(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, RunnerError> {
    match &state.kubernetes {
        Some(backend) => Ok(Json(
            backend
                .profile()
                .await
                .map_err(RunnerError::admission_unavailable)?,
        )),
        None => Err(RunnerError::not_found("runner backend profile unavailable")),
    }
}

async fn kubernetes_scope(
    State(state): State<AppState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RunnerError> {
    authenticate(&state, &headers, &method, &uri, &[]).await?;
    let backend = state
        .kubernetes
        .as_ref()
        .ok_or_else(|| RunnerError::not_found("Kubernetes backend absent"))?;
    Ok(Json(
        backend
            .scope_status()
            .await
            .map_err(RunnerError::admission_unavailable)?,
    ))
}
async fn kubernetes_quiesce(
    State(state): State<AppState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, RunnerError> {
    authenticate(&state, &headers, &method, &uri, &body).await?;
    let request: kubernetes::ScopeRequest = serde_json::from_slice(&body)
        .map_err(|_| RunnerError::bad_request("invalid Kubernetes scope request"))?;
    let backend = state
        .kubernetes
        .as_ref()
        .ok_or_else(|| RunnerError::not_found("Kubernetes backend absent"))?;
    Ok(Json(
        backend
            .quiesce(&request)
            .await
            .map_err(RunnerError::admission_unavailable)?,
    ))
}
async fn kubernetes_activate(
    State(state): State<AppState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, RunnerError> {
    authenticate(&state, &headers, &method, &uri, &body).await?;
    let request: kubernetes::ScopeRequest = serde_json::from_slice(&body)
        .map_err(|_| RunnerError::bad_request("invalid Kubernetes scope request"))?;
    let backend = state
        .kubernetes
        .as_ref()
        .ok_or_else(|| RunnerError::not_found("Kubernetes backend absent"))?;
    Ok(Json(
        backend
            .activate(&request)
            .await
            .map_err(RunnerError::admission_unavailable)?,
    ))
}
async fn validate_sandbox_security(config: &Config) -> Result<()> {
    let security = docker(&["info", "--format", "{{json .SecurityOptions}}"])
        .await
        .context("failed to inspect Docker security options")?;
    let options: Vec<String> = serde_json::from_slice(&security.stdout)
        .context("Docker returned invalid security options")?;
    if !options.iter().any(|option| option.contains("seccomp")) {
        bail!("Docker daemon does not advertise seccomp support");
    }
    if config.require_apparmor && !options.iter().any(|option| option.contains("apparmor")) {
        bail!("COWORK_SANDBOX_REQUIRE_APPARMOR is enabled but Docker does not advertise AppArmor support");
    }

    let mut probe = vec![
        "run".to_owned(),
        "--rm".to_owned(),
        "--network".to_owned(),
        "none".to_owned(),
        "--read-only".to_owned(),
        "--cap-drop".to_owned(),
        "ALL".to_owned(),
        "--security-opt".to_owned(),
        "no-new-privileges:true".to_owned(),
    ];
    append_sandbox_security_options(&mut probe, config);
    probe.extend([config.core_image.clone(), "true".to_owned()]);
    let output = docker_owned_with_stdin(probe, None)
        .await
        .context("failed to start the sandbox security probe")?;
    if !output.status.success() {
        bail!(
            "sandbox security probe was rejected: {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(2_000)
                .collect::<String>()
        );
    }
    tracing::info!(
        seccomp = %config.seccomp_profile,
        apparmor = ?config.apparmor_profile,
        "sandbox security profiles verified"
    );
    Ok(())
}

fn append_sandbox_security_options(args: &mut Vec<String>, config: &Config) {
    // Docker applies its built-in profile by default; "builtin" is not a file.
    // Startup verifies seccomp support and executes a sandbox probe before serving.
    if config.seccomp_profile != "builtin" {
        args.push("--security-opt".to_owned());
        args.push(format!("seccomp={}", config.seccomp_profile));
    }
    if let Some(profile) = &config.apparmor_profile {
        args.push("--security-opt".to_owned());
        args.push(format!("apparmor={profile}"));
    }
}

async fn recover_desktop_sessions() -> Result<HashMap<uuid::Uuid, SandboxDesktopSessionResult>> {
    let name_filter = format!("name=^/{}", scoped_resource("desktop", ""));
    let listed = docker(&[
        "ps",
        "--filter",
        "label=dev.opencowork.runner_managed=true",
        "--filter",
        &name_filter,
        "--format",
        "{{.ID}}",
    ])
    .await?;
    let mut recovered = HashMap::new();
    for container_id in String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        match inspect_desktop_session(container_id).await {
            Ok(session) => {
                if recovered.insert(session.session_id, session).is_some() {
                    tracing::warn!(%container_id, "discarding duplicate desktop session container");
                    let _ = docker(&["kill", container_id]).await;
                }
            }
            Err(error) => {
                tracing::warn!(%container_id, ?error, "discarding malformed desktop session container");
                let _ = docker(&["kill", container_id]).await;
            }
        }
    }
    Ok(recovered)
}

async fn inspect_desktop_session(container_id: &str) -> Result<SandboxDesktopSessionResult> {
    let inspected = docker(&["inspect", container_id]).await?;
    let entries: Vec<serde_json::Value> = serde_json::from_slice(&inspected.stdout)?;
    let entry = entries
        .first()
        .context("Docker inspect returned no container")?;
    let labels = entry
        .pointer("/Config/Labels")
        .and_then(serde_json::Value::as_object)
        .context("desktop container labels are missing")?;
    let parse_label = |name: &str| -> Result<&str> {
        labels
            .get(name)
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("desktop container label {name} is missing"))
    };
    let session_id = parse_label("dev.opencowork.desktop_session")?.parse()?;
    let run_id = parse_label("dev.opencowork.run_id")?.parse()?;
    let container_name = entry
        .get("Name")
        .and_then(serde_json::Value::as_str)
        .context("desktop container name is missing")?
        .trim_start_matches('/')
        .to_owned();
    let workspace_volume = entry
        .get("Mounts")
        .and_then(serde_json::Value::as_array)
        .and_then(|mounts| {
            mounts.iter().find_map(|mount| {
                (mount.get("Destination")?.as_str()? == "/workspace")
                    .then(|| mount.get("Name")?.as_str().map(str::to_owned))?
            })
        })
        .context("desktop workspace volume is missing")?;
    let environment = entry
        .pointer("/Config/Env")
        .and_then(serde_json::Value::as_array)
        .context("desktop container environment is missing")?;
    let size = environment
        .iter()
        .filter_map(serde_json::Value::as_str)
        .find_map(|value| value.strip_prefix("COWORK_DESKTOP_SIZE="))
        .context("desktop dimensions are missing")?;
    let mut parts = size.split('x');
    let width = parts.next().context("desktop width is missing")?.parse()?;
    let height = parts.next().context("desktop height is missing")?.parse()?;
    let scale_factor = labels
        .get("dev.opencowork.desktop_scale")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("1")
        .parse()?;
    Ok(SandboxDesktopSessionResult {
        schema_version: SCHEMA_VERSION,
        session_id,
        run_id,
        container_name,
        workspace_volume,
        dimensions: DesktopDimensions {
            width,
            height,
            scale_factor,
        },
    })
}

async fn run_job(
    State(state): State<AppState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<SandboxRunResult>, RunnerError> {
    authenticate(&state, &headers, &method, &uri, &body).await?;
    let mut value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|error| RunnerError::bad_request(format!("invalid run spec: {error}")))?;
    let execution: Option<admission::Execution> = value
        .as_object_mut()
        .ok_or_else(|| RunnerError::bad_request("invalid run spec"))?
        .remove("_execution")
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| RunnerError::bad_request(format!("invalid execution binding: {error}")))?;
    let spec: SandboxRunSpec = serde_json::from_value(value)
        .map_err(|error| RunnerError::bad_request(format!("invalid run spec: {error}")))?;
    validate_workspace_route(&uri, spec.workspace_id)?;
    validate_spec(&state.config, &spec)?;
    if let Some(backend) = &state.kubernetes {
        return backend
            .execute(&state.config, &spec, execution.as_ref(), &body)
            .await
            .map(Json);
    }
    if let Some(execution) = &execution {
        state
            .admission
            .begin_execution(spec.run_id, spec.workspace_id, execution, &body)
            .await?;
    }
    match execute(&state, &spec, execution.as_ref()).await {
        Ok(result) => Ok(Json(result)),
        Err(mut error) => {
            // Only the admission module can mint this pre-reservation marker.
            // Generic 429, conflict, Docker and accounting errors remain unproven.
            if let Some(marker) = &error.no_start {
                let proof = admission::no_start_proof(
                    marker,
                    &state.config.signing_key,
                    method.as_str(),
                    uri.path_and_query()
                        .map(|value| value.as_str())
                        .unwrap_or_else(|| uri.path()),
                    header(&headers, "x-cowork-timestamp")?,
                    header(&headers, "x-cowork-nonce")?,
                    &body,
                )?;
                error.no_start_proof = Some(proof);
            }
            Err(error)
        }
    }
}

async fn stop_execution(
    State(state): State<AppState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, RunnerError> {
    authenticate(&state, &headers, &method, &uri, &body).await?;
    let request: admission::StopRequest = serde_json::from_slice(&body)
        .map_err(|error| RunnerError::bad_request(format!("invalid stop request: {error}")))?;
    let receipt = match &state.kubernetes {
        Some(backend) => backend.stop(&request).await?,
        None => state.admission.stop_execution(&request).await?,
    };
    let response = serde_json::to_vec(&receipt)
        .map_err(|error| RunnerError::admission_unavailable(error.into()))?;
    let proof = execution_stop_proof(
        &state.config.signing_key,
        method.as_str(),
        uri.path(),
        header(&headers, "x-cowork-timestamp")?,
        header(&headers, "x-cowork-nonce")?,
        &body,
        &response,
    )?;
    Ok((
        [
            ("content-type", "application/json"),
            ("x-cowork-stop-proof", proof.as_str()),
        ],
        response,
    )
        .into_response())
}

fn execution_stop_proof(
    key: &[u8],
    method: &str,
    path: &str,
    timestamp: &str,
    nonce: &str,
    request: &[u8],
    response: &[u8],
) -> Result<String, RunnerError> {
    use sha2::Digest;
    let request_hash = hex::encode(Sha256::digest(request));
    let response_hash = hex::encode(Sha256::digest(response));
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| RunnerError::internal("invalid runner signing key"))?;
    mac.update(b"open-cowork/runner-stop/v1\0");
    for field in [
        method,
        path,
        timestamp,
        nonce,
        request_hash.as_str(),
        "200",
        response_hash.as_str(),
    ] {
        mac.update(&(field.len() as u64).to_be_bytes());
        mac.update(field.as_bytes());
    }
    Ok(format!("v1:{}", hex::encode(mac.finalize().into_bytes())))
}

#[derive(Debug, Deserialize)]
struct RunFileQuery {
    path: String,
    workspace_id: Option<uuid::Uuid>,
}

async fn read_run_file(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
    Query(query): Query<RunFileQuery>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, RunnerError> {
    authenticate(&state, &headers, &method, &uri, &[]).await?;
    validate_workspace_route(&uri, query.workspace_id)?;
    let path = validated_workspace_file(&query.path)?;
    if !path.starts_with("artifacts/") {
        return Err(RunnerError::bad_request(
            "only files below artifacts/ can be exported",
        ));
    }
    if let Some(backend) = &state.kubernetes {
        let request = uuid::Uuid::parse_str(header(&headers, "x-cowork-nonce")?)
            .map_err(|_| RunnerError::unauthorized("invalid original transfer request"))?;
        let bytes = backend
            .read_artifact(
                &state.config,
                run_id,
                query.workspace_id,
                &path,
                request,
                uri.path_and_query()
                    .map(|v| v.as_str())
                    .unwrap_or(uri.path()),
            )
            .await?;
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/octet-stream")
            .header("content-length", bytes.len())
            .body(Body::from(bytes))
            .map_err(|e| RunnerError::internal(e.to_string()));
    }
    let volume = workspace_volume(run_id, query.workspace_id);
    let script = "import pathlib,sys; root=pathlib.Path('/workspace').resolve(); p=(root/sys.argv[1]).resolve(strict=True); assert root in p.parents and p.is_file(),'artifact not found'; size=p.stat().st_size; assert size <= 67108864,'artifact exceeds the 64 MiB transfer limit'; sys.stdout.buffer.write(p.read_bytes())";
    let container_name = scoped_resource("transfer", &uuid::Uuid::new_v4().simple().to_string());
    let mut args = vec![
        "run".to_owned(),
        "--rm".to_owned(),
        "--name".to_owned(),
        container_name.clone(),
        "--network".to_owned(),
        "none".to_owned(),
        "--user".to_owned(),
        "10001:10001".to_owned(),
        "--read-only".to_owned(),
        "--cap-drop".to_owned(),
        "ALL".to_owned(),
        "--security-opt".to_owned(),
        "no-new-privileges:true".to_owned(),
        "--pids-limit".to_owned(),
        "64".to_owned(),
        "--memory".to_owned(),
        "256m".to_owned(),
        "--cpus".to_owned(),
        "0.5".to_owned(),
        "--volume".to_owned(),
        format!("{volume}:/workspace:ro"),
    ];
    append_sandbox_security_options(&mut args, &state.config);
    args.extend([
        state.config.core_image.clone(),
        "python3".to_owned(),
        "-c".to_owned(),
        script.to_owned(),
        path,
    ]);
    let (permit, container_id) = admitted_create(
        &state,
        args,
        container_name,
        run_id,
        Resources {
            cpu_nanos: 500_000_000,
            memory_bytes: 256 * 1024 * 1024,
        },
        None,
    )
    .await?;
    let output =
        docker_owned_with_stdin(vec!["start".into(), "--attach".into(), container_id], None)
            .await
            .map_err(RunnerError::docker)?;
    state.admission.finished(&permit).await?;
    if !output.status.success() {
        return Err(RunnerError::not_found(format!(
            "artifact is unavailable: {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(500)
                .collect::<String>()
        )));
    }
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/octet-stream")
        .header("content-length", output.stdout.len())
        .body(Body::from(output.stdout))
        .map_err(|error| RunnerError::internal(error.to_string()))
}

#[derive(Debug, Deserialize)]
struct TerminalQuery {
    workspace_id: Option<uuid::Uuid>,
    columns: u16,
    rows: u16,
}

async fn stream_terminal(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
    Query(query): Query<TerminalQuery>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, RunnerError> {
    authenticate(&state, &headers, &method, &uri, &[]).await?;
    validate_workspace_route(&uri, query.workspace_id)?;
    if !(20..=400).contains(&query.columns) || !(5..=200).contains(&query.rows) {
        return Err(RunnerError::bad_request(
            "terminal dimensions are outside the supported range",
        ));
    }
    let volume = workspace_volume(run_id, query.workspace_id);
    if let Some(backend) = &state.kubernetes {
        let backend = backend.clone();
        let config = state.config.clone();
        let request = uuid::Uuid::parse_str(header(&headers, "x-cowork-nonce")?)
            .map_err(|_| RunnerError::unauthorized("original terminal request nonce invalid"))?;
        let route = uri
            .path_and_query()
            .map_or(uri.path(), |v| v.as_str())
            .to_owned();
        return Ok(upgrade.on_upgrade(move |socket| async move {
            if let Err(e) = backend
                .terminal(
                    config,
                    socket,
                    run_id,
                    query.workspace_id,
                    request,
                    route,
                    query.columns,
                    query.rows,
                )
                .await
            {
                tracing::warn!(?e, "native terminal closed");
            }
        }));
    }
    if docker(&["volume", "inspect", &volume]).await.is_err() {
        return Err(RunnerError::not_found("the run workspace is not available"));
    }
    Ok(upgrade.on_upgrade(move |socket| {
        relay_terminal(
            socket,
            state,
            run_id,
            query.workspace_id,
            query.columns,
            query.rows,
        )
    }))
}

async fn relay_terminal(
    socket: WebSocket,
    state: AppState,
    run_id: uuid::Uuid,
    workspace_id: Option<uuid::Uuid>,
    columns: u16,
    rows: u16,
) {
    if let Err(error) =
        relay_terminal_inner(socket, &state, run_id, workspace_id, columns, rows).await
    {
        tracing::warn!(?error, %run_id, "terminal stream ended with an error");
    }
}

async fn relay_terminal_inner(
    mut socket: WebSocket,
    state: &AppState,
    run_id: uuid::Uuid,
    workspace_id: Option<uuid::Uuid>,
    columns: u16,
    rows: u16,
) -> Result<()> {
    let config = &state.config;
    let container_name = scoped_resource("terminal", &uuid::Uuid::new_v4().simple().to_string());
    let volume = workspace_volume(run_id, workspace_id);
    let terminal_size = format!("stty columns {columns} rows {rows} 2>/dev/null || true");
    let shell = format!(
        "{terminal_size}; if command -v bash >/dev/null 2>&1; then exec bash -l; else exec sh -l; fi"
    );
    let mut args = vec![
        "run".to_owned(),
        "--rm".to_owned(),
        "--interactive".to_owned(),
        "--name".to_owned(),
        container_name.clone(),
        "--label".to_owned(),
        "dev.opencowork.terminal_managed=true".to_owned(),
        "--label".to_owned(),
        format!("dev.opencowork.run_id={run_id}"),
        "--network".to_owned(),
        "none".to_owned(),
        "--user".to_owned(),
        "10001:10001".to_owned(),
        "--read-only".to_owned(),
        "--cap-drop".to_owned(),
        "ALL".to_owned(),
        "--security-opt".to_owned(),
        "no-new-privileges:true".to_owned(),
        "--pids-limit".to_owned(),
        "256".to_owned(),
        "--memory".to_owned(),
        "1g".to_owned(),
        "--cpus".to_owned(),
        "1".to_owned(),
        "--tmpfs".to_owned(),
        "/tmp:rw,noexec,nosuid,nodev,size=268435456".to_owned(),
        "--volume".to_owned(),
        format!("{volume}:/workspace:rw"),
        "--workdir".to_owned(),
        "/workspace".to_owned(),
        "--env".to_owned(),
        "TERM=xterm-256color".to_owned(),
    ];
    append_sandbox_security_options(&mut args, config);
    args.extend([
        config.core_image.clone(),
        "/usr/bin/script".to_owned(),
        "-qefc".to_owned(),
        shell,
        "/dev/null".to_owned(),
    ]);
    let (permit, container_id) = match admitted_create(
        state,
        args,
        container_name,
        run_id,
        Resources {
            cpu_nanos: 1_000_000_000,
            memory_bytes: 1024 * 1024 * 1024,
        },
        None,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            socket
                .send(Message::Binary(
                    format!("\r\n{}: {}\r\n", error.code, error.message)
                        .into_bytes()
                        .into(),
                ))
                .await?;
            socket.close().await?;
            return Ok(());
        }
    };
    let mut child = Command::new("docker")
        .args(["start", "--attach", "--interactive", &container_id])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to start terminal sandbox")?;
    let mut input = child.stdin.take().context("terminal stdin is missing")?;
    let mut output = child.stdout.take().context("terminal stdout is missing")?;
    let mut error_output = child.stderr.take().context("terminal stderr is missing")?;
    let (mut sender, mut receiver) = socket.split();
    let mut stdout_buffer = vec![0_u8; 16 * 1024];
    let mut stderr_buffer = vec![0_u8; 4 * 1024];
    let outcome: Result<()> = async {
        loop {
            tokio::select! {
                read = output.read(&mut stdout_buffer) => {
                    let count = read?;
                    if count == 0 { break; }
                    sender.send(Message::Binary(stdout_buffer[..count].to_vec().into())).await?;
                }
                read = error_output.read(&mut stderr_buffer) => {
                    let count = read?;
                    if count > 0 {
                        sender.send(Message::Binary(stderr_buffer[..count].to_vec().into())).await?;
                    }
                }
                incoming = receiver.next() => match incoming {
                    Some(Ok(Message::Binary(bytes))) if bytes.len() <= 64 * 1024 => input.write_all(&bytes).await?,
                    Some(Ok(Message::Text(text))) if text.len() <= 64 * 1024 => input.write_all(text.as_bytes()).await?,
                    Some(Ok(Message::Ping(bytes))) => sender.send(Message::Pong(bytes)).await?,
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(error)) => return Err(error.into()),
                    _ => {}
                }
            }
        }
        Ok(())
    }
    .await;
    drop(input);
    let _ = child.kill().await;
    let _ = docker(&["rm", "--force", &container_id]).await;
    state
        .admission
        .finished(&permit)
        .await
        .map_err(|error| anyhow::anyhow!(error.message))?;
    outcome
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DesktopStreamQuery {
    #[serde(default)]
    control: bool,
    binding_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DesktopStopQuery {
    binding_sha256: Option<String>,
}

fn require_desktop_proofs(state: &AppState) -> Result<(), RunnerError> {
    if !state.desktop_job_proofs {
        return Err(RunnerError::resource_conflict(
            "private desktop job proofs are disabled",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn desktop_proof_response(
    state: &AppState,
    domain: &[u8],
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    request: &[u8],
    response: Vec<u8>,
    binding: Option<Vec<u8>>,
) -> Result<Response, RunnerError> {
    if response.len() > 64 * 1024 || binding.as_ref().is_some_and(|value| value.len() > 8 * 1024) {
        return Err(RunnerError::internal(
            "desktop proof exceeds response bounds",
        ));
    }
    let mut proof = admission::proof_response(
        domain,
        &state.config.signing_key,
        [
            method.as_str(),
            uri.path_and_query()
                .map_or(uri.path(), |value| value.as_str()),
            header(headers, "x-cowork-timestamp")?,
            header(headers, "x-cowork-nonce")?,
        ],
        request,
        &response,
        binding.as_deref(),
    )
    .map_err(RunnerError::admission_unavailable)?;
    if domain.starts_with(b"open-cowork/runner-kubernetes-") {
        proof = proof.replacen("v1:", "v2:", 1);
    }
    let mut response = (
        [
            ("content-type", "application/json"),
            ("x-cowork-desktop-proof", proof.as_str()),
        ],
        response,
    )
        .into_response();
    if let Some(binding) = binding {
        response.headers_mut().insert(
            "x-cowork-desktop-binding",
            URL_SAFE_NO_PAD
                .encode(binding)
                .parse()
                .map_err(|_| RunnerError::internal("invalid desktop binding header"))?,
        );
    }
    Ok(response)
}

async fn probe_desktop_session(
    State(state): State<AppState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, RunnerError> {
    authenticate(&state, &headers, &method, &uri, &body).await?;
    if let Some(backend) = &state.kubernetes {
        if !state.kubernetes_desktop_proofs {
            return Err(RunnerError::resource_conflict(
                "native desktop proofs disabled",
            ));
        }
        let request: kubernetes::NativeProbeRequest = serde_json::from_slice(&body)
            .map_err(|_| RunnerError::bad_request("invalid original native desktop probe"))?;
        let result = backend
            .probe_desktop(&request)
            .await
            .map_err(RunnerError::admission_unavailable)?;
        return desktop_proof_response(
            &state,
            kubernetes::PROBE_DOMAIN,
            &method,
            &uri,
            &headers,
            &body,
            serde_json::to_vec(&result).map_err(|e| RunnerError::internal(e.to_string()))?,
            None,
        );
    }
    require_desktop_proofs(&state)?;
    let request: admission::ProbeRequest = serde_json::from_slice(&body)
        .map_err(|error| RunnerError::bad_request(format!("invalid desktop probe: {error}")))?;
    let result = state
        .admission
        .probe_desktop(&request)
        .await
        .map_err(RunnerError::admission_unavailable)?;
    desktop_proof_response(
        &state,
        admission::PROBE_DOMAIN,
        &method,
        &uri,
        &headers,
        &body,
        serde_json::to_vec(&result)
            .map_err(|error| RunnerError::admission_unavailable(error.into()))?,
        None,
    )
}

async fn start_desktop_session(
    State(state): State<AppState>,
    Query(query): Query<admission::CreationQuery>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, RunnerError> {
    authenticate(&state, &headers, &method, &uri, &body).await?;
    let spec: SandboxDesktopSessionSpec = serde_json::from_slice(&body)
        .map_err(|error| RunnerError::bad_request(format!("invalid desktop spec: {error}")))?;
    validate_workspace_route(&uri, spec.workspace_id)?;
    ensure_compatible(spec.schema_version)
        .map_err(|error| RunnerError::bad_request(error.to_string()))?;
    if !(640..=3840).contains(&spec.dimensions.width)
        || !(480..=2160).contains(&spec.dimensions.height)
        || !(0.5..=4.0).contains(&spec.dimensions.scale_factor)
    {
        return Err(RunnerError::bad_request(
            "desktop dimensions are outside the allowed range",
        ));
    }
    if spec.network != SandboxNetwork::FilteredEgress
        || state.config.filtered_egress_network.is_none()
        || state.config.filtered_egress_proxy.is_none()
    {
        return Err(RunnerError::bad_request(
            "desktop sessions require the filtered-egress network and proxy",
        ));
    }
    if state
        .desktop_sessions
        .lock()
        .await
        .contains_key(&spec.session_id)
    {
        return Err(RunnerError::bad_request("desktop session already exists"));
    }

    let private = query
        .private()
        .map_err(|error| RunnerError::bad_request(error.to_string()))?;
    if let Some(backend) = &state.kubernetes {
        if !state.kubernetes_desktop_proofs {
            return Err(RunnerError::resource_conflict(
                "native desktop proofs disabled",
            ));
        }
        let (lease, attempt) = private.ok_or_else(|| {
            RunnerError::bad_request("original native desktop lease and attempt required")
        })?;
        let (result, job) = backend
            .create_desktop(
                &state.config,
                &spec,
                lease,
                attempt,
                &body,
                uri.path_and_query().map_or(uri.path(), |v| v.as_str()),
            )
            .await
            .map_err(RunnerError::admission_unavailable)?;
        return desktop_proof_response(
            &state,
            kubernetes::CREATE_DOMAIN,
            &method,
            &uri,
            &headers,
            &body,
            serde_json::to_vec(&result).map_err(|e| RunnerError::internal(e.to_string()))?,
            Some(serde_json::to_vec(&job).map_err(|e| RunnerError::internal(e.to_string()))?),
        );
    }
    if private.is_some() || state.desktop_job_proofs {
        require_desktop_proofs(&state)?;
        let (lease, attempt) = private.ok_or_else(|| {
            RunnerError::bad_request("private desktop creation needs lease and attempt identities")
        })?;
        state
            .admission
            .begin_execution(
                spec.run_id,
                spec.workspace_id,
                &admission::Execution {
                    version: 1,
                    lease_token: lease,
                    attempt_id: attempt,
                },
                &body,
            )
            .await?;
        state
            .admission
            .prepare_desktop_identity(&spec, lease, attempt)
            .await
            .map_err(RunnerError::admission_unavailable)?;
    }
    let result = launch_desktop(&state, &spec, private).await?;
    state
        .desktop_sessions
        .lock()
        .await
        .insert(spec.session_id, result.clone());
    if let Some((lease, attempt)) = private {
        let job = state
            .admission
            .desktop_creation_receipt(&spec, lease, attempt)
            .await
            .map_err(RunnerError::admission_unavailable)?;
        desktop_proof_response(
            &state,
            admission::CREATE_DOMAIN,
            &method,
            &uri,
            &headers,
            &body,
            serde_json::to_vec(&result)
                .map_err(|error| RunnerError::admission_unavailable(error.into()))?,
            Some(
                serde_json::to_vec(&job)
                    .map_err(|error| RunnerError::admission_unavailable(error.into()))?,
            ),
        )
    } else {
        Ok(Json(result).into_response())
    }
}

async fn stop_desktop_session(
    State(state): State<AppState>,
    Path(session_id): Path<uuid::Uuid>,
    Query(query): Query<DesktopStopQuery>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, RunnerError> {
    authenticate(&state, &headers, &method, &uri, &[]).await?;
    if let Some(backend) = &state.kubernetes {
        if !state.kubernetes_desktop_proofs {
            return Err(RunnerError::resource_conflict(
                "native desktop proofs disabled",
            ));
        }
        let digest = query.binding_sha256.ok_or_else(|| {
            RunnerError::bad_request("original native desktop binding digest required")
        })?;
        let result = backend
            .stop_desktop(session_id, &digest)
            .await
            .map_err(RunnerError::admission_unavailable)?;
        return desktop_proof_response(
            &state,
            kubernetes::STOP_DOMAIN,
            &method,
            &uri,
            &headers,
            &[],
            serde_json::to_vec(&result).map_err(|e| RunnerError::internal(e.to_string()))?,
            None,
        );
    }
    if query.binding_sha256.is_some() || state.desktop_job_proofs {
        require_desktop_proofs(&state)?;
        let digest = query.binding_sha256.ok_or_else(|| {
            RunnerError::bad_request("desktop stop requires the physical job binding")
        })?;
        let result = state
            .admission
            .stop_bound_desktop(session_id, &digest)
            .await
            .map_err(RunnerError::admission_unavailable)?;
        state.desktop_sessions.lock().await.remove(&session_id);
        return desktop_proof_response(
            &state,
            admission::STOP_DOMAIN,
            &method,
            &uri,
            &headers,
            &[],
            serde_json::to_vec(&result)
                .map_err(|error| RunnerError::admission_unavailable(error.into()))?,
            None,
        );
    }
    let session = state
        .desktop_sessions
        .lock()
        .await
        .get(&session_id)
        .cloned()
        .ok_or_else(|| RunnerError::not_found("desktop session was not found"))?;
    docker(&["rm", "--force", &session.container_name])
        .await
        .map_err(RunnerError::docker)?;
    state.desktop_sessions.lock().await.remove(&session_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn stream_desktop_session(
    State(state): State<AppState>,
    Path(session_id): Path<uuid::Uuid>,
    Query(query): Query<DesktopStreamQuery>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, RunnerError> {
    authenticate(&state, &headers, &method, &uri, &[]).await?;
    if let Some(backend) = &state.kubernetes {
        if !state.kubernetes_desktop_proofs {
            return Err(RunnerError::resource_conflict(
                "native desktop proofs disabled",
            ));
        }
        let digest = query.binding_sha256.ok_or_else(|| {
            RunnerError::bad_request("original native desktop binding digest required")
        })?;
        let binding = backend
            .desktop_binding(session_id, &digest)
            .await
            .map_err(RunnerError::admission_unavailable)?;
        let backend = backend.clone();
        return Ok(upgrade.on_upgrade(move |socket| async move {
            if let Err(e) = backend.relay_desktop(socket, binding, query.control).await {
                tracing::warn!(?e, "native desktop stream closed");
            }
        }));
    }
    if query.binding_sha256.is_some() || state.desktop_job_proofs {
        require_desktop_proofs(&state)?;
        let digest = query.binding_sha256.ok_or_else(|| {
            RunnerError::bad_request("desktop stream requires the physical job binding")
        })?;
        // Authenticate the handshake before upgrading; check again immediately
        // before exec, after the HTTP upgrade has actually completed.
        state
            .admission
            .desktop_stream_id(session_id, &digest)
            .await
            .map_err(RunnerError::admission_unavailable)?;
        return Ok(upgrade.on_upgrade(move |socket| async move {
            match state.admission.desktop_stream_id(session_id, &digest).await {
                Ok(id) => relay_vnc(socket, id, query.control).await,
                Err(error) => {
                    tracing::warn!(?error, %session_id, "desktop binding changed before RFB exec")
                }
            }
        }));
    }
    let session = state
        .desktop_sessions
        .lock()
        .await
        .get(&session_id)
        .cloned()
        .ok_or_else(|| RunnerError::not_found("desktop session was not found"))?;
    Ok(upgrade.on_upgrade(move |socket| relay_vnc(socket, session.container_name, query.control)))
}

async fn launch_desktop(
    state: &AppState,
    spec: &SandboxDesktopSessionSpec,
    private: Option<(uuid::Uuid, uuid::Uuid)>,
) -> Result<SandboxDesktopSessionResult, RunnerError> {
    let config = &state.config;
    let compact_session = spec.session_id.simple().to_string();

    let container_name = scoped_resource("desktop", &compact_session);
    let resources = Resources {
        cpu_nanos: spec.limits.cpu_nanos.clamp(100_000_000, 16_000_000_000),
        memory_bytes: spec
            .limits
            .memory_bytes
            .clamp(512 * 1024 * 1024, 32 * 1024 * 1024 * 1024),
    };
    let workspace_volume = workspace_volume(spec.run_id, spec.workspace_id);
    let memory = spec
        .limits
        .memory_bytes
        .clamp(512 * 1024 * 1024, 32 * 1024 * 1024 * 1024)
        .to_string();
    let cpus = format!(
        "{:.9}",
        spec.limits.cpu_nanos.clamp(100_000_000, 16_000_000_000) as f64 / 1_000_000_000_f64
    );
    let pids = spec.limits.pids.clamp(64, 4096).to_string();
    let proxy = config
        .filtered_egress_proxy
        .as_ref()
        .expect("validated proxy");
    let mut args = vec![
        "run".to_owned(),
        "--rm".to_owned(),
        "--name".to_owned(),
        container_name.clone(),
        "--label".to_owned(),
        "dev.opencowork.runner_managed=true".to_owned(),
        "--label".to_owned(),
        format!("dev.opencowork.run_id={}", spec.run_id),
        "--label".to_owned(),
        format!("dev.opencowork.desktop_session={}", spec.session_id),
        "--label".to_owned(),
        format!(
            "dev.opencowork.desktop_scale={}",
            spec.dimensions.scale_factor
        ),
        "--user".to_owned(),
        "10001:10001".to_owned(),
        "--read-only".to_owned(),
        "--cap-drop".to_owned(),
        "ALL".to_owned(),
        "--security-opt".to_owned(),
        "no-new-privileges:true".to_owned(),
        "--pids-limit".to_owned(),
        pids,
        "--memory".to_owned(),
        memory,
        "--cpus".to_owned(),
        cpus,
        "--shm-size".to_owned(),
        "1g".to_owned(),
        "--tmpfs".to_owned(),
        format!(
            "/tmp:rw,noexec,nosuid,nodev,size={}",
            spec.limits
                .tmpfs_bytes
                .clamp(64 * 1024 * 1024, 4 * 1024 * 1024 * 1024)
        ),
        "--volume".to_owned(),
        format!("{workspace_volume}:/workspace:rw"),
        "--network".to_owned(),
        config
            .filtered_egress_network
            .clone()
            .expect("validated network"),
        "--env".to_owned(),
        format!(
            "COWORK_DESKTOP_SIZE={}x{}x24",
            spec.dimensions.width, spec.dimensions.height
        ),
    ];
    append_sandbox_security_options(&mut args, config);
    if let Some((lease, attempt)) = private {
        for label in [
            format!("dev.opencowork.desktop_lease={lease}"),
            format!("dev.opencowork.desktop_attempt={attempt}"),
            format!(
                "dev.opencowork.workspace_id={}",
                spec.workspace_id
                    .map_or_else(|| "none".into(), |id| id.to_string())
            ),
        ] {
            args.extend(["--label".into(), label]);
        }
    }
    for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
        args.push("--env".to_owned());
        args.push(format!("{key}={proxy}"));
    }
    for key in ["NO_PROXY", "no_proxy"] {
        args.push("--env".to_owned());
        args.push(format!("{key}=localhost,127.0.0.1,::1"));
    }
    args.extend([
        config.gui_image.clone(),
        "/bin/bash".to_owned(),
        "-lc".to_owned(),
        "while true; do sleep 3600; done".to_owned(),
    ]);
    let execution = private.map(|(lease, attempt)| admission::Execution {
        version: 1,
        lease_token: lease,
        attempt_id: attempt,
    });
    let (permit, container_id) = admitted_create(
        state,
        args,
        container_name.clone(),
        spec.run_id,
        resources,
        execution.as_ref(),
    )
    .await?;
    if private.is_some() {
        state
            .admission
            .bind_desktop_container(spec.session_id, &permit, &container_id)
            .await
            .map_err(RunnerError::admission_unavailable)?;
    }
    docker(&["start", &container_id])
        .await
        .map_err(RunnerError::docker)?;
    let mut ready = false;
    for _ in 0..300 {
        let probe = docker(&[
            "exec",
            &container_id,
            "/bin/bash",
            "-lc",
            "exec 3<>/dev/tcp/127.0.0.1/5900",
        ])
        .await;
        if probe.is_ok() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !ready {
        let container_logs = docker(&["logs", &container_id])
            .await
            .map(|output| {
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                )
            })
            .unwrap_or_else(|error| format!("unable to read container logs: {error:#}"));
        let capture_logs = docker(&[
            "exec",
            &container_id,
            "/bin/bash",
            "-lc",
            "for file in /tmp/xvfb.log /tmp/openbox.log /tmp/x11vnc-control.log /tmp/x11vnc-view.log; do printf '\n[%s]\n' \"$file\"; tail -n 80 \"$file\" 2>&1 || true; done",
        ])
        .await
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_else(|error| format!("unable to read capture logs: {error:#}"));
        let _ = docker(&["kill", &container_id]).await;
        return Err(RunnerError::internal(format!(
            "desktop capture did not become ready; container={container_logs}; capture={capture_logs}"
        )));
    }
    Ok(SandboxDesktopSessionResult {
        schema_version: SCHEMA_VERSION,
        session_id: spec.session_id,
        run_id: spec.run_id,
        container_name,
        workspace_volume,
        dimensions: spec.dimensions.clone(),
    })
}

async fn relay_vnc(socket: WebSocket, container_name: String, control: bool) {
    if let Err(error) = relay_vnc_inner(socket, &container_name, control).await {
        tracing::warn!(?error, %container_name, "desktop stream ended with an error");
    }
}

async fn relay_vnc_inner(socket: WebSocket, container_name: &str, control: bool) -> Result<()> {
    let port = if control { 5900 } else { 5901 };
    let mut child = Command::new("docker")
        .args([
            "exec",
            "--interactive",
            container_name,
            "socat",
            "STDIO",
            &format!("TCP:127.0.0.1:{port}"),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut input = child
        .stdin
        .take()
        .context("desktop relay stdin is unavailable")?;
    let mut output = child
        .stdout
        .take()
        .context("desktop relay stdout is unavailable")?;
    let (mut sender, mut receiver) = socket.split();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        tokio::select! {
            read = output.read(&mut buffer) => {
                let read = read?;
                if read == 0 { break; }
                sender.send(Message::Binary(buffer[..read].to_vec().into())).await?;
            }
            incoming = receiver.next() => {
                match incoming {
                    Some(Ok(Message::Binary(bytes))) => input.write_all(&bytes).await?,
                    Some(Ok(Message::Ping(bytes))) => sender.send(Message::Pong(bytes)).await?,
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(error)) => return Err(error.into()),
                    _ => {}
                }
            }
        }
    }
    let _ = child.kill().await;
    Ok(())
}

async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    method: &Method,
    uri: &Uri,
    body: &[u8],
) -> Result<(), RunnerError> {
    let timestamp = header(headers, "x-cowork-timestamp")?
        .parse::<i64>()
        .map_err(|_| RunnerError::unauthorized("invalid signature timestamp"))?;
    let signature = header(headers, "x-cowork-signature")?;
    let nonce = header(headers, "x-cowork-nonce")?;
    uuid::Uuid::parse_str(nonce)
        .map_err(|_| RunnerError::unauthorized("signature nonce is not a UUID"))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RunnerError::internal("system clock is before Unix epoch"))?
        .as_secs() as i64;
    if (now - timestamp).abs() > 30 {
        return Err(RunnerError::unauthorized(
            "signature timestamp is outside the 30 second window",
        ));
    }
    let supplied = hex::decode(signature)
        .map_err(|_| RunnerError::unauthorized("signature is not valid hexadecimal"))?;
    let mut mac = HmacSha256::new_from_slice(&state.config.signing_key)
        .map_err(|_| RunnerError::internal("invalid signing key"))?;
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b"\n");
    mac.update(nonce.as_bytes());
    mac.update(b"\n");
    mac.update(method.as_str().as_bytes());
    mac.update(b"\n");
    mac.update(
        uri.path_and_query()
            .map(|value| value.as_str())
            .unwrap_or_else(|| uri.path())
            .as_bytes(),
    );
    mac.update(b"\n");
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    if supplied.len() != expected.len() || !bool::from(supplied.ct_eq(expected.as_slice())) {
        return Err(RunnerError::unauthorized("signature verification failed"));
    }

    let mut replay_cache = state.replay_cache.lock().await;
    replay_cache.retain(|_, seen_at| now - *seen_at <= 30);
    if replay_cache.insert(nonce.to_owned(), now).is_some() {
        return Err(RunnerError::unauthorized("signed request was already used"));
    }
    Ok(())
}

fn validated_workspace_file(path: &str) -> Result<String, RunnerError> {
    if path.is_empty()
        || path.len() > 4096
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(RunnerError::bad_request(
            "artifact path must be a normalized relative POSIX path",
        ));
    }
    Ok(path.to_owned())
}

fn validate_workspace_route(uri: &Uri, workspace: Option<uuid::Uuid>) -> Result<(), RunnerError> {
    if uri.path().starts_with("/v2/") != workspace.is_some()
        || workspace.is_some_and(|id| id.is_nil())
    {
        return Err(RunnerError::bad_request(
            "chat workspace requires a non-nil binding and the v2 runner endpoint",
        ));
    }
    Ok(())
}

fn workspace_volume(run: uuid::Uuid, workspace: Option<uuid::Uuid>) -> String {
    // Preserve the inventory format used by backup/restore/deletion. The UUID
    // suffix is a storage identity; with v2 it need not be an individual run ID.
    scoped_resource("run", &workspace.unwrap_or(run).simple().to_string())
}

fn validate_spec(config: &Config, spec: &SandboxRunSpec) -> Result<(), RunnerError> {
    ensure_compatible(spec.schema_version)
        .map_err(|error| RunnerError::bad_request(error.to_string()))?;
    if spec.argv.is_empty() || spec.argv.len() > 64 {
        return Err(RunnerError::bad_request(
            "argv must contain 1 to 64 entries",
        ));
    }
    if spec
        .argv
        .iter()
        .any(|arg| arg.len() > 32_768 || arg.contains('\0'))
    {
        return Err(RunnerError::bad_request("argv contains an invalid entry"));
    }
    if spec.environment.len() > 64
        || spec.environment.iter().any(|(key, value)| {
            key.is_empty()
                || key.len() > 128
                || value.len() > 32_768
                || value.contains('\0')
                || !key
                    .chars()
                    .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        })
    {
        return Err(RunnerError::bad_request(
            "environment contains an invalid entry",
        ));
    }
    if let Some(stdin) = &spec.stdin_base64 {
        if stdin.len() > 24 * 1024 * 1024
            || STANDARD
                .decode(stdin)
                .map(|bytes| bytes.len() > 16 * 1024 * 1024)
                .unwrap_or(true)
        {
            return Err(RunnerError::bad_request(
                "stdin_base64 must contain at most 16 MiB of valid base64",
            ));
        }
    }
    if spec.limits.memory_bytes < 64 * 1024 * 1024
        || spec.limits.memory_bytes > 32 * 1024 * 1024 * 1024
        || spec.limits.cpu_nanos < 100_000_000
        || spec.limits.cpu_nanos > 16_000_000_000
        || spec.limits.pids == 0
        || spec.limits.pids > 4096
        || spec.limits.timeout_seconds == 0
        || spec.limits.timeout_seconds > 24 * 60 * 60
        || spec.limits.tmpfs_bytes > 4 * 1024 * 1024 * 1024
        || spec.limits.output_bytes > 32 * 1024 * 1024
    {
        return Err(RunnerError::bad_request(
            "sandbox limits are outside the allowed range",
        ));
    }
    if spec.network == SandboxNetwork::FilteredEgress
        && (config.filtered_egress_network.is_none() || config.filtered_egress_proxy.is_none())
    {
        return Err(RunnerError::bad_request(
            "filtered egress was requested but no filtered network and proxy are configured",
        ));
    }
    if spec.environment.keys().any(|key| {
        matches!(
            key.to_ascii_uppercase().as_str(),
            "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY" | "NO_PROXY"
        )
    }) {
        return Err(RunnerError::bad_request(
            "proxy environment variables are reserved by the runner",
        ));
    }
    Ok(())
}

async fn execute(
    state: &AppState,
    spec: &SandboxRunSpec,
    execution: Option<&admission::Execution>,
) -> Result<SandboxRunResult, RunnerError> {
    if spec.image == SandboxImage::Gui {
        let active = state
            .desktop_sessions
            .lock()
            .await
            .values()
            .find(|session| session.run_id == spec.run_id)
            .cloned();
        if let Some(session) = active {
            if session.workspace_volume != workspace_volume(spec.run_id, spec.workspace_id) {
                return Err(RunnerError::bad_request(
                    "desktop workspace binding differs",
                ));
            }
            let container_id = state
                .admission
                .desktop_execution(spec.run_id, &session.container_name, execution)
                .await?;
            return execute_in_desktop(spec, &session, &container_id).await;
        }
    }
    let config = &state.config;
    let compact_id = spec.run_id.simple().to_string();
    let container_name = scoped_resource("job", &compact_id);
    let workspace_volume = workspace_volume(spec.run_id, spec.workspace_id);

    let image = match spec.image {
        SandboxImage::Core => &config.core_image,
        SandboxImage::Gui => &config.gui_image,
        SandboxImage::Crew => &config.crew_image,
    };
    let memory = spec.limits.memory_bytes.to_string();
    let cpus = format!("{:.9}", spec.limits.cpu_nanos as f64 / 1_000_000_000_f64);
    let pids = spec.limits.pids.to_string();
    let tmpfs = format!(
        "/tmp:rw,noexec,nosuid,nodev,size={}",
        spec.limits.tmpfs_bytes
    );
    let mount = format!("{workspace_volume}:/workspace:rw");
    let mut args = vec![
        "run".to_owned(),
        "--rm".to_owned(),
        "--name".to_owned(),
        container_name.clone(),
        "--label".to_owned(),
        format!("dev.opencowork.run_id={}", spec.run_id),
        "--user".to_owned(),
        "10001:10001".to_owned(),
        "--read-only".to_owned(),
        "--cap-drop".to_owned(),
        "ALL".to_owned(),
        "--security-opt".to_owned(),
        "no-new-privileges:true".to_owned(),
        "--pids-limit".to_owned(),
        pids,
        "--memory".to_owned(),
        memory,
        "--cpus".to_owned(),
        cpus,
        "--tmpfs".to_owned(),
        tmpfs,
        "--volume".to_owned(),
        mount,
        "--network".to_owned(),
        match spec.network {
            SandboxNetwork::None => "none".to_owned(),
            SandboxNetwork::FilteredEgress => config
                .filtered_egress_network
                .clone()
                .expect("validated filtered network"),
        },
    ];
    append_sandbox_security_options(&mut args, config);
    if spec.stdin_base64.is_some() {
        args.insert(1, "--interactive".to_owned());
    }
    for (key, value) in &spec.environment {
        args.push("--env".to_owned());
        args.push(format!("{key}={value}"));
    }
    if spec.network == SandboxNetwork::FilteredEgress {
        let proxy = config
            .filtered_egress_proxy
            .as_ref()
            .expect("validated filtered proxy");
        for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            args.push("--env".to_owned());
            args.push(format!("{key}={proxy}"));
        }
        args.push("--env".to_owned());
        args.push("NO_PROXY=localhost,127.0.0.1,::1".to_owned());
    }
    args.push(image.clone());
    args.extend(spec.argv.iter().cloned());

    let stdin = spec
        .stdin_base64
        .as_deref()
        .map(|value| STANDARD.decode(value))
        .transpose()
        .map_err(|_| RunnerError::bad_request("stdin_base64 is invalid"))?;
    let (permit, container_id) = admitted_create(
        state,
        args,
        container_name.clone(),
        spec.run_id,
        Resources {
            cpu_nanos: spec.limits.cpu_nanos,
            memory_bytes: spec.limits.memory_bytes,
        },
        execution,
    )
    .await?;
    let mut start = vec!["start".into(), "--attach".into()];
    if stdin.is_some() {
        start.push("--interactive".into());
    }
    start.push(container_id.clone());
    let output = tokio::time::timeout(
        Duration::from_secs(spec.limits.timeout_seconds),
        docker_owned_with_stdin(start, stdin),
    )
    .await;
    let (exit_code, timed_out, stdout, stderr) = match output {
        Ok(Ok(output)) => {
            state.admission.finished(&permit).await?;
            (output.status.code(), false, output.stdout, output.stderr)
        }
        Ok(Err(error)) => return Err(RunnerError::docker(error)),
        Err(_) => {
            let _ = docker(&["rm", "--force", &container_id]).await;
            state.admission.finished(&permit).await?;
            (
                None,
                true,
                Vec::new(),
                b"sandbox exceeded its time limit".to_vec(),
            )
        }
    };
    let max = usize::try_from(spec.limits.output_bytes).unwrap_or(usize::MAX);
    let output_truncated = stdout.len() > max || stderr.len() > max;
    Ok(SandboxRunResult {
        schema_version: SCHEMA_VERSION,
        run_id: spec.run_id,
        container_name,
        workspace_volume,
        exit_code,
        timed_out,
        stdout: String::from_utf8_lossy(&stdout[..stdout.len().min(max)]).into_owned(),
        stderr: String::from_utf8_lossy(&stderr[..stderr.len().min(max)]).into_owned(),
        output_truncated,
    })
}

async fn execute_in_desktop(
    spec: &SandboxRunSpec,
    session: &SandboxDesktopSessionResult,
    container_id: &str,
) -> Result<SandboxRunResult, RunnerError> {
    let mut args = vec![
        "exec".to_owned(),
        "--workdir".to_owned(),
        "/workspace".to_owned(),
    ];
    if spec.stdin_base64.is_some() {
        args.push("--interactive".to_owned());
    }
    for (key, value) in &spec.environment {
        args.push("--env".to_owned());
        args.push(format!("{key}={value}"));
    }
    args.push(container_id.to_owned());
    args.extend([
        "timeout".to_owned(),
        "--signal=KILL".to_owned(),
        format!("{}s", spec.limits.timeout_seconds),
    ]);
    args.extend(spec.argv.iter().cloned());
    let stdin = spec
        .stdin_base64
        .as_deref()
        .map(|value| STANDARD.decode(value))
        .transpose()
        .map_err(|_| RunnerError::bad_request("stdin_base64 is invalid"))?;
    let output = tokio::time::timeout(
        Duration::from_secs(spec.limits.timeout_seconds.saturating_add(5)),
        docker_owned_with_stdin(args, stdin),
    )
    .await;
    let (exit_code, timed_out, stdout, stderr) = match output {
        Ok(Ok(output)) => (output.status.code(), false, output.stdout, output.stderr),
        Ok(Err(error)) => return Err(RunnerError::docker(error)),
        Err(_) => (
            None,
            true,
            Vec::new(),
            b"desktop command exceeded its time limit".to_vec(),
        ),
    };
    let max = usize::try_from(spec.limits.output_bytes).unwrap_or(usize::MAX);
    let output_truncated = stdout.len() > max || stderr.len() > max;
    Ok(SandboxRunResult {
        schema_version: SCHEMA_VERSION,
        run_id: spec.run_id,
        container_name: session.container_name.clone(),
        workspace_volume: session.workspace_volume.clone(),
        exit_code,
        timed_out,
        stdout: String::from_utf8_lossy(&stdout[..stdout.len().min(max)]).into_owned(),
        stderr: String::from_utf8_lossy(&stderr[..stderr.len().min(max)]).into_owned(),
        output_truncated,
    })
}

async fn docker(args: &[&str]) -> Result<std::process::Output> {
    let output = Command::new("docker")
        .kill_on_drop(true)
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        bail!("docker failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(output)
}

async fn admitted_create(
    state: &AppState,
    mut args: Vec<String>,
    container_name: String,
    run_id: uuid::Uuid,
    resources: Resources,
    execution: Option<&admission::Execution>,
) -> Result<(admission::Permit, String), RunnerError> {
    let permit = match execution {
        Some(execution) => {
            state
                .admission
                .reserve_execution(container_name, run_id, resources, Some(execution))
                .await?
        }
        None => {
            state
                .admission
                .reserve(container_name, run_id, resources)
                .await?
        }
    };
    args[0] = "create".to_owned();
    permit.annotate(&mut args);
    let output = docker_owned_with_stdin(args, None)
        .await
        .map_err(RunnerError::docker)?;
    // Persist the exact Docker ID before allowing execution. A crash before this
    // point leaves a conservative, durable pending reservation.
    if !output.status.success() {
        return Err(RunnerError::docker(anyhow::anyhow!(
            "sandbox creation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if id.len() != 64 || !id.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(RunnerError::internal(
            "Docker returned an invalid container identity",
        ));
    }
    state.admission.created(&permit, id.clone()).await?;
    Ok((permit, id))
}

async fn docker_owned_with_stdin(
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
) -> Result<std::process::Output> {
    let mut command = Command::new("docker");
    command.kill_on_drop(true);
    command.args(args);
    if stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let mut child = command.spawn()?;
    if let Some(stdin) = stdin {
        let mut pipe = child.stdin.take().context("failed to open docker stdin")?;
        pipe.write_all(&stdin).await?;
        pipe.shutdown().await?;
    }
    Ok(child.wait_with_output().await?)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, RunnerError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| RunnerError::unauthorized(format!("missing {name} header")))
}

impl Config {
    fn from_env() -> Result<Self> {
        let signing_key = secret("COWORK_RUNNER_SIGNING_KEY")?.into_bytes();
        if signing_key.len() < 32 {
            bail!("COWORK_RUNNER_SIGNING_KEY must contain at least 32 characters");
        }
        let seccomp_profile = security_profile(
            "COWORK_SANDBOX_SECCOMP_PROFILE",
            env::var("COWORK_SANDBOX_SECCOMP_PROFILE").unwrap_or_else(|_| "builtin".to_owned()),
        )?;
        let apparmor_profile = env::var("COWORK_SANDBOX_APPARMOR_PROFILE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| security_profile("COWORK_SANDBOX_APPARMOR_PROFILE", value))
            .transpose()?;
        let require_apparmor = env_flag("COWORK_SANDBOX_REQUIRE_APPARMOR", false)?;
        if require_apparmor && apparmor_profile.is_none() {
            bail!("COWORK_SANDBOX_REQUIRE_APPARMOR requires COWORK_SANDBOX_APPARMOR_PROFILE");
        }
        Ok(Self {
            listen_addr: env::var("COWORK_RUNNER_LISTEN_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:8090".to_owned())
                .parse()
                .context("invalid COWORK_RUNNER_LISTEN_ADDR")?,
            signing_key,
            core_image: env::var("COWORK_RUNNER_CORE_IMAGE")
                .unwrap_or_else(|_| "open-cowork-sandbox-core:0.3.0".to_owned()),
            gui_image: env::var("COWORK_RUNNER_GUI_IMAGE")
                .unwrap_or_else(|_| "open-cowork-sandbox-gui:0.3.0".to_owned()),
            crew_image: env::var("COWORK_RUNNER_CREW_IMAGE")
                .unwrap_or_else(|_| "open-cowork-sandbox-crew:0.3.0".to_owned()),
            filtered_egress_network: env::var("COWORK_SANDBOX_EGRESS_NETWORK")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            filtered_egress_proxy: env::var("COWORK_SANDBOX_HTTP_PROXY")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            seccomp_profile,
            apparmor_profile,
            require_apparmor,
        })
    }
}

fn scoped_resource(kind: &str, id: &str) -> String {
    let instance = env::var("COWORK_RUNNER_INSTANCE_ID").ok();
    if kind == "run" {
        if let (Some(instance), Ok(generation)) =
            (&instance, env::var("COWORK_RUNNER_STORAGE_GENERATION"))
        {
            let instance = uuid::Uuid::parse_str(instance).expect("validated instance ID");
            let generation = uuid::Uuid::parse_str(&generation).expect("validated generation ID");
            return format!(
                "cowork-{}-g-{}-run-{id}",
                instance.simple(),
                generation.simple()
            );
        }
    }
    scoped_resource_for(instance.as_deref(), kind, id)
}

fn scoped_resource_for(instance: Option<&str>, kind: &str, id: &str) -> String {
    match instance {
        Some(instance) => {
            // Configuration is validated before the runner accepts requests.
            let instance = uuid::Uuid::parse_str(instance).expect("validated runner instance ID");
            format!("cowork-{}-{kind}-{id}", instance.simple())
        }
        None => format!("cowork-{kind}-{id}"),
    }
}

fn security_profile(name: &str, value: String) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        bail!("{name} must be a non-empty single-line value");
    }
    if value.eq_ignore_ascii_case("unconfined") {
        bail!("{name}=unconfined is forbidden");
    }
    Ok(value.to_owned())
}

fn env_flag(name: &str, default: bool) -> Result<bool> {
    let Ok(value) = env::var(name) else {
        return Ok(default);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => bail!("{name} must be true or false"),
    }
}

fn secret(name: &str) -> Result<String> {
    if let Ok(value) = env::var(name) {
        if !value.trim().is_empty() {
            return Ok(value);
        }
    }
    let file_var = format!("{name}_FILE");
    let path = env::var(&file_var).with_context(|| format!("missing {name} or {file_var}"))?;
    let value =
        fs::read_to_string(&path).with_context(|| format!("failed to read secret file {path}"))?;
    Ok(value.trim().to_owned())
}

#[derive(Debug)]
struct RunnerError {
    status: StatusCode,
    code: &'static str,
    message: String,
    no_start: Option<admission::NoStart>,
    no_start_proof: Option<String>,
}

impl RunnerError {
    fn resource_conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "sandbox_launch_unproven",
            message: message.into(),
            no_start: None,
            no_start_proof: None,
        }
    }

    fn admission_unavailable(error: anyhow::Error) -> Self {
        tracing::error!(
            ?error,
            "sandbox admission unavailable; resources remain reserved"
        );
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "sandbox_admission_unavailable",
            message: "sandbox resource accounting is unavailable; operator review may be required"
                .into(),
            no_start: None,
            no_start_proof: None,
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_run_spec",
            message: message.into(),
            no_start: None,
            no_start_proof: None,
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: message.into(),
            no_start: None,
            no_start_proof: None,
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
            no_start: None,
            no_start_proof: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: message.into(),
            no_start: None,
            no_start_proof: None,
        }
    }

    fn docker(error: anyhow::Error) -> Self {
        tracing::error!(?error, "Docker runner operation failed");
        Self::internal("sandbox execution failed")
    }
}

impl IntoResponse for RunnerError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(ErrorBody {
                error: self.code,
                message: self.message,
            }),
        )
            .into_response();
        if let Some(proof) = self.no_start_proof {
            // Proofs are generated as a fixed ASCII version and hexadecimal MAC.
            if let Ok(value) = proof.parse() {
                response
                    .headers_mut()
                    .insert("x-cowork-no-start-proof", value);
            }
        }
        response
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "cowork_runner=info,tower_http=info".into());
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().json())
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_stop_response_uses_independent_request_bound_fixed_vector() {
        let request=br#"{"version":1,"run_id":"00000000-0000-4000-8000-000000000002","lease_token":null,"cancel_lease":null,"attempts":[]}"#;
        let response=br#"{"version":1,"run_id":"00000000-0000-4000-8000-000000000002","lease_token":null,"cancel_lease":null,"attempts":[],"containers":[],"execution_stopped":true}"#;
        let proof = execution_stop_proof(
            &[b'k'; 32],
            "POST",
            "/v1/execution-stop",
            "1700000000",
            "00000000-0000-4000-8000-000000000001",
            request,
            response,
        )
        .unwrap();
        assert_eq!(
            proof,
            "v1:4d5ee56fc94d759dd2261b87b79cf41dbbfa08b262b4fa2e7f5c88ccd520393a"
        );
        assert_ne!(
            proof,
            execution_stop_proof(
                &[b'k'; 32],
                "POST",
                "/v1/execution-stop",
                "1700000000",
                "different",
                request,
                response
            )
            .unwrap()
        );
    }

    #[test]
    fn workspace_binding_cannot_silently_downgrade_runner_protocol() {
        let workspace = uuid::Uuid::new_v4();
        for resource in [
            "jobs",
            "desktop-sessions",
            "runs/unused/file",
            "runs/unused/terminal",
        ] {
            let legacy: Uri = format!("/v1/{resource}").parse().unwrap();
            let current: Uri = format!("/v2/{resource}").parse().unwrap();
            assert!(validate_workspace_route(&legacy, None).is_ok());
            assert!(validate_workspace_route(&current, Some(workspace)).is_ok());
            assert!(validate_workspace_route(&legacy, Some(workspace)).is_err());
            assert!(validate_workspace_route(&current, None).is_err());
            assert!(validate_workspace_route(&current, Some(uuid::Uuid::nil())).is_err());
        }
    }

    #[test]
    fn same_chat_reuses_storage_and_other_chats_remain_separate() {
        let first = uuid::Uuid::new_v4();
        let next = uuid::Uuid::new_v4();
        let workspace = uuid::Uuid::new_v4();
        assert_eq!(
            workspace_volume(first, Some(workspace)),
            workspace_volume(next, Some(workspace))
        );
        assert_ne!(workspace_volume(first, None), workspace_volume(next, None));
        assert_ne!(
            workspace_volume(first, Some(workspace)),
            workspace_volume(first, Some(uuid::Uuid::new_v4()))
        );
    }

    #[test]
    fn instance_scopes_separate_identical_run_ids_and_legacy_volumes() {
        let a = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let b = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
        assert_ne!(
            scoped_resource_for(Some(a), "run", "same"),
            scoped_resource_for(Some(b), "run", "same")
        );
        assert_ne!(
            scoped_resource_for(Some(a), "run", "same"),
            scoped_resource_for(None, "run", "same")
        );
        assert_eq!(scoped_resource_for(None, "run", "same"), "cowork-run-same");
    }

    fn test_config() -> Config {
        Config {
            listen_addr: "127.0.0.1:8090".parse().unwrap(),
            signing_key: vec![1; 32],
            core_image: "core:test".to_owned(),
            gui_image: "gui:test".to_owned(),
            crew_image: "crew:test".to_owned(),
            filtered_egress_network: None,
            filtered_egress_proxy: None,
            seccomp_profile: "builtin".to_owned(),
            apparmor_profile: Some("open-cowork-sandbox".to_owned()),
            require_apparmor: true,
        }
    }

    #[test]
    fn builtin_seccomp_uses_docker_default_and_custom_profiles_are_explicit() {
        let mut args = Vec::new();
        append_sandbox_security_options(&mut args, &test_config());
        assert_eq!(args, ["--security-opt", "apparmor=open-cowork-sandbox"]);
        let mut custom = test_config();
        custom.seccomp_profile = "/profiles/seccomp.json".into();
        args.clear();
        append_sandbox_security_options(&mut args, &custom);
        assert_eq!(args[1], "seccomp=/profiles/seccomp.json");
    }

    #[test]
    fn unconfined_security_profiles_are_forbidden() {
        assert!(security_profile("SECCOMP", "unconfined".to_owned()).is_err());
        assert!(security_profile("APPARMOR", "UNCONFINED".to_owned()).is_err());
    }
}
