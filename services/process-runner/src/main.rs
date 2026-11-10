//! Independent authenticated receiver for the durable V2 process protocol.
//! Launch admission, physical ownership and completion are distinct durable steps.
mod agent_sandbox;
mod cleanup;
mod config;
mod platform;
#[cfg(windows)]
#[path = "../../daemon/src/private_secret_file.rs"]
mod private_secret_file;
mod sandbox_lifecycle;
#[cfg(windows)]
mod windows_orphan;
mod workspace;

use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use config::Configuration;
use cowork_contracts::{ExecutionEnvironmentRef, ProcessResult};
use cowork_domain::{
    canonical_hash,
    environments::{EnvironmentObservation, EnvironmentQualification},
};
use cowork_runner_protocol::{
    Admission, Binding, Broker, EnvironmentInstall, ExecutionLeaseRenewal, ExecutionState,
    FenceRequest, LaunchRequest, Signed, SigningKey,
};
use hyper_util::rt::TokioIo;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    path::{Path as FsPath, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpListener, sync::Semaphore};
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

#[derive(Clone)]
struct App(Arc<Inner>);
struct Inner {
    config: Configuration,
    broker: Broker,
    signer: SigningKey,
    token_tag: Vec<u8>,
    token_key: ring::hmac::Key,
    serial: Arc<Semaphore>,
    stopping: AtomicBool,
}
type ApiResult<T> = std::result::Result<T, ApiError>;
struct ApiError(StatusCode, &'static str);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"error":self.1}))).into_response()
    }
}
fn failure(_: impl std::fmt::Display) -> ApiError {
    ApiError(
        StatusCode::CONFLICT,
        "Execution or qualification remains unconfirmed",
    )
}
fn auth(app: &App, headers: &HeaderMap) -> ApiResult<()> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError(
            StatusCode::UNAUTHORIZED,
            "Runner authority required",
        ))?;
    ring::hmac::verify(&app.0.token_key, token.as_bytes(), &app.0.token_tag)
        .map_err(|_| ApiError(StatusCode::UNAUTHORIZED, "Runner authority denied"))
}
fn key(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c)),
        "Invalid durable identifier"
    );
    Ok(())
}
fn environment_path(config: &Configuration, id: &str, version: &str) -> PathBuf {
    config
        .data_directory
        .join("environments")
        .join(format!("{}.json", canonical_hash(&(id, version))))
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Installed {
    request: Signed<EnvironmentInstall>,
    qualification: Signed<EnvironmentQualification>,
}
fn installed(app: &App, reference: &ExecutionEnvironmentRef) -> Result<Installed> {
    installed_record(app, reference, true)
}
fn installed_record(
    app: &App,
    reference: &ExecutionEnvironmentRef,
    fresh: bool,
) -> Result<Installed> {
    let record: Installed = serde_json::from_slice(&config::private_read(
        &environment_path(
            &app.0.config,
            &reference.environment_id,
            &reference.version_id,
        ),
        1024 * 1024,
    )?)?;
    record.request.verify(
        "environment_install",
        &app.0.config.binding.control_public_key,
    )?;
    record.qualification.verify(
        "environment_qualification",
        &app.0.config.binding.runner_public_key,
    )?;
    ensure!(
        record.request.payload.environment_id == reference.environment_id
            && record.request.payload.version_id == reference.version_id
            && canonical_hash(&record.request.payload.definition) == reference.definition_sha256
            && canonical_hash(&record.qualification.payload) == reference.qualification_sha256,
        "Frozen environment identity changed"
    );
    ensure!(
        record.qualification.payload.runner_id == app.0.config.binding.runner_id
            && record.qualification.payload.runner_generation == app.0.config.binding.generation
            && record.qualification.payload.adapter_build_sha256
                == app.0.config.binding.adapter_build_sha256,
        "Installed runner qualification changed"
    );
    if fresh {
        platform::inspect(&app.0.config, &record.request.payload.definition)?;
    }
    Ok(record)
}
async fn fence(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<Signed<FenceRequest>>,
) -> ApiResult<StatusCode> {
    auth(&app, &headers)?;
    app.0.broker.fence(request).await.map_err(failure)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn renew(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<Signed<ExecutionLeaseRenewal>>,
) -> ApiResult<StatusCode> {
    auth(&app, &headers)?;
    app.0
        .broker
        .renew_execution_lease(request)
        .await
        .map_err(failure)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn observe(
    State(app): State<App>,
    headers: HeaderMap,
    Json(reference): Json<ExecutionEnvironmentRef>,
) -> ApiResult<Json<Signed<EnvironmentObservation>>> {
    auth(&app, &headers)?;
    let a = app.clone();
    let record = tokio::task::spawn_blocking(move || installed(&a, &reference))
        .await
        .map_err(failure)?
        .map_err(failure)?;
    let q = record.qualification.payload;
    Ok(Json(
        app.0
            .signer
            .sign(
                "environment_observation",
                EnvironmentObservation {
                    runner_id: q.runner_id,
                    runner_generation: q.runner_generation,
                    adapter_build_sha256: q.adapter_build_sha256,
                    artifact_sha256: q.artifact_sha256,
                    definition_sha256: q.definition_sha256,
                    capabilities: q.capabilities,
                    observed_at: platform::now(),
                },
            )
            .map_err(failure)?,
    ))
}
async fn install(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<Signed<EnvironmentInstall>>,
) -> ApiResult<Json<Signed<EnvironmentQualification>>> {
    auth(&app, &headers)?;
    request
        .verify(
            "environment_install",
            &app.0.config.binding.control_public_key,
        )
        .map_err(failure)?;
    key(&request.payload.environment_id).map_err(failure)?;
    key(&request.payload.version_id).map_err(failure)?;
    request.payload.definition.validate().map_err(failure)?;
    if app.0.stopping.load(Ordering::Acquire) {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Receiver is stopping",
        ));
    }
    let path = environment_path(
        &app.0.config,
        &request.payload.environment_id,
        &request.payload.version_id,
    );
    if path.exists() {
        let record: Installed =
            serde_json::from_slice(&config::private_read(&path, 1024 * 1024).map_err(failure)?)
                .map_err(failure)?;
        record
            .request
            .verify(
                "environment_install",
                &app.0.config.binding.control_public_key,
            )
            .map_err(failure)?;
        record
            .qualification
            .verify(
                "environment_qualification",
                &app.0.config.binding.runner_public_key,
            )
            .map_err(failure)?;
        if record.request != request {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "Environment version identity is immutable",
            ));
        }
        return Ok(Json(record.qualification));
    }
    let permit = app
        .0
        .serial
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::CONFLICT, "Receiver is busy"))?;
    let config = app.0.config.clone();
    let definition = request.payload.definition.clone();
    let attempt = config
        .data_directory
        .join("attempts")
        .join(format!("qualification-{}", uuid::Uuid::new_v4()));
    cleanup::capacity(&config).map_err(failure)?;
    config::private_directory(&attempt).map_err(failure)?;
    // The detached worker owns qualification through completion even if HTTP is
    // canceled. Installation never accepts client-supplied capability evidence.
    let a = app.clone();
    let failure_state = app.clone();
    let task = tokio::spawn(async move {
        let _permit = permit;
        let result = async {
            let proof_attempt = attempt.clone();
            let measured = tokio::task::spawn_blocking(move || {
                platform::qualify(&config, &definition, &attempt)
            })
            .await??;
            let q = a.0.signer.sign(
                "environment_qualification",
                EnvironmentQualification {
                    runner_id: a.0.config.binding.runner_id.clone(),
                    runner_generation: a.0.config.binding.generation,
                    adapter_build_sha256: a.0.config.binding.adapter_build_sha256.clone(),
                    artifact_sha256: request.payload.definition.artifact.sha256().into(),
                    definition_sha256: canonical_hash(&request.payload.definition),
                    report_sha256: measured.0,
                    capabilities: measured.1,
                    verified_at: platform::now(),
                },
            )?;
            let installed = Installed {
                request,
                qualification: q.clone(),
            };
            config::durable_json(
                &proof_attempt.join("qualification-outcome.json"),
                &installed,
            )?;
            config::durable_json(&path, &installed)?;
            cleanup::finish(
                &a,
                &proof_attempt,
                cleanup::Proof::Qualification { installed },
            )
            .await?;
            Ok::<_, anyhow::Error>(q)
        }
        .await;
        if result.is_err() {
            a.0.stopping.store(true, Ordering::Release);
        }
        result
    });
    match task.await {
        Ok(Ok(result)) => Ok(Json(result)),
        failed => {
            failure_state.0.stopping.store(true, Ordering::Release);
            Err(failure(format!(
                "Qualification termination unconfirmed: {}",
                failed.is_err()
            )))
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PhysicalOutcome {
    request: LaunchRequest,
    result: ProcessResult,
}
async fn launch(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<Signed<LaunchRequest>>,
) -> ApiResult<Response> {
    auth(&app, &headers)?;
    if app.0.stopping.load(Ordering::Acquire) {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Receiver is stopping",
        ));
    }
    // Immutable replay uses the broker's retained receipt and never enters the
    // adapter again, including requests whose initial HTTP reply was lost.
    request
        .verify("launch", &app.0.config.binding.control_public_key)
        .map_err(failure)?;
    if let Ok(previous) = app
        .0
        .broker
        .receipt(&request.payload.process_attempt_id)
        .await
    {
        if previous.payload.request != request.payload
            || previous.payload.request_envelope_sha256
                != cowork_runner_protocol::envelope_hash(&request)
        {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "Execution identity conflicts",
            ));
        }
        return Ok((
            if matches!(previous.payload.state, ExecutionState::Completed { .. }) {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            },
            Json(previous),
        )
            .into_response());
    }
    let a = app.clone();
    let reference = request.payload.environment.clone();
    let environment = tokio::task::spawn_blocking(move || installed(&a, &reference))
        .await
        .map_err(failure)?
        .map_err(failure)?;
    let serial = app
        .0
        .serial
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::CONFLICT, "Receiver is busy"))?;
    cleanup::capacity(&app.0.config).map_err(failure)?;
    match app.0.broker.admit(request).await.map_err(failure)? {
        Admission::Existing(receipt) => Ok((
            if matches!(receipt.payload.state, ExecutionState::Completed { .. }) {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            },
            Json(receipt),
        )
            .into_response()),
        Admission::Launch(permit) => {
            let dispatched = app.0.broker.dispatch_owned(permit).await.map_err(failure)?;
            let request = dispatched.request().clone();
            let receiver = app.clone();
            tokio::spawn(async move {
                let _serial = serial;
                let cancel = Arc::new(AtomicBool::new(false));
                let worker_cancel = cancel.clone();
                let config = receiver.0.config.clone();
                let definition = environment.request.payload.definition;
                let worker_request = request.clone();
                let worker = tokio::task::spawn_blocking(move || -> Result<ProcessResult> {
                    // Retain the non-reconstructable handoff until the actual
                    // worker starts. Dropped HTTP futures do not own this worker;
                    // a crash never turns its retained receipt into a new token.
                    let _dispatched = dispatched;
                    let attempt = config
                        .data_directory
                        .join("attempts")
                        .join(canonical_hash(&worker_request));
                    config::private_directory(&attempt)?;
                    config::durable_json(&attempt.join("request.json"), &worker_request)?;
                    let workspace = workspace::Workspace::acquire(
                        &config.workspace_state_directory,
                        &worker_request.workspace_resource_id,
                    )?;
                    let input = attempt.join("input");
                    workspace::copy(&workspace.root, &input, &workspace.before)?;
                    ensure!(
                        !worker_cancel.load(Ordering::Acquire),
                        "Execution authority was canceled before launch"
                    );
                    let (outcome, output) = platform::run_workspace(
                        &config,
                        &definition,
                        &worker_request.invocation,
                        &input,
                        &attempt,
                        &worker_cancel,
                        &worker_request.workspace_resource_id,
                    )?;
                    // Promotion belongs to this one retained physical attempt. A
                    // cancellation can still have produced files and is not undone.
                    workspace.promote(&output, &attempt.join("promotion.json"))?;
                    Ok(platform::result(outcome))
                });
                tokio::pin!(worker);
                let result = loop {
                    tokio::select! {
                        result=&mut worker=>break result,
                        _=tokio::time::sleep(Duration::from_millis(250))=>{
                            if receiver.0.stopping.load(Ordering::Acquire)||platform::now()>=request.execution_deadline
                                ||!receiver.0.broker.execution_allowed(&request).await.unwrap_or(false){cancel.store(true,Ordering::Release);}
                        }
                    }
                };
                match result {
                    Ok(Ok(result)) => {
                        let attempt = receiver
                            .0
                            .config
                            .data_directory
                            .join("attempts")
                            .join(canonical_hash(&request));
                        let settled = (|| -> Result<()> {
                            let physical = receiver.0.signer.sign(
                                "physical_outcome",
                                PhysicalOutcome {
                                    request: request.clone(),
                                    result: result.clone(),
                                },
                            )?;
                            config::durable_json(&attempt.join("physical-outcome.json"), &physical)
                        })();
                        if settled.is_ok() {
                            if receiver
                                .0
                                .broker
                                .complete(
                                    &request.process_attempt_id,
                                    &request.request_sha256,
                                    result.clone(),
                                )
                                .await
                                .is_err()
                            {
                                eprintln!(
                                    "Retained physical outcome requires reconciliation for {}",
                                    request.process_attempt_id
                                );
                            } else {
                                let proof = receiver.0.signer.sign(
                                    "physical_outcome",
                                    PhysicalOutcome {
                                        request: request.clone(),
                                        result,
                                    },
                                );
                                if let Ok(outcome) = proof
                                    && cleanup::finish(
                                        &receiver,
                                        &attempt,
                                        cleanup::Proof::Execution { outcome },
                                    )
                                    .await
                                    .is_err()
                                {
                                    eprintln!(
                                        "Completed resources require safe cleanup for {}",
                                        request.process_attempt_id
                                    );
                                }
                            }
                        } else {
                            eprintln!(
                                "Physical outcome durability remains uncertain for {}",
                                request.process_attempt_id
                            );
                        }
                    }
                    _ => {
                        receiver.0.stopping.store(true, Ordering::Release);
                        eprintln!(
                            "Physical execution remains uncertain for {}; receiver pauses for owned recovery",
                            request.process_attempt_id
                        );
                    }
                }
            });
            Ok(StatusCode::ACCEPTED.into_response())
        }
    }
}
async fn receipt(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    auth(&app, &headers)?;
    let receipt = app.0.broker.receipt(&id).await.map_err(failure)?;
    Ok((
        if matches!(receipt.payload.state, ExecutionState::Completed { .. }) {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        },
        Json(receipt),
    )
        .into_response())
}
fn pem(bytes: &[u8], label: &str) -> Result<Vec<Vec<u8>>> {
    let text = std::str::from_utf8(bytes)?;
    let start = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let mut remaining = text;
    let mut result = Vec::new();
    while let Some((_, tail)) = remaining.split_once(&start) {
        let (body, next) = tail.split_once(&end).context("Malformed TLS PEM")?;
        result.push(
            STANDARD.decode(
                body.chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>(),
            )?,
        );
        remaining = next;
        ensure!(result.len() <= 16, "TLS chain is oversized");
    }
    ensure!(!result.is_empty(), "TLS PEM block missing");
    Ok(result)
}
async fn recovery(app: &App) -> Result<()> {
    sandbox_lifecycle::recover(&app.0.config)?;
    cleanup::collect_garbage(app).await?;
    for entry in std::fs::read_dir(app.0.config.data_directory.join("attempts"))? {
        let entry = entry?;
        config::ordinary(&entry.path(), true)?;
        let owner = entry.path().join("ownership.json");
        if owner.exists() {
            cleanup::recover_owned(&app.0.config, &entry.path())?;
        }
        let retained = entry.path().join("physical-outcome.json");
        if retained.exists() {
            let outcome: Signed<PhysicalOutcome> =
                serde_json::from_slice(&config::private_read(&retained, 2 * 1024 * 1024)?)?;
            outcome.verify("physical_outcome", &app.0.config.binding.runner_public_key)?;
            let receipt = app
                .0
                .broker
                .receipt(&outcome.payload.request.process_attempt_id)
                .await?;
            ensure!(
                receipt.payload.request == outcome.payload.request,
                "Physical outcome request differs from durable admission"
            );
            app.0
                .broker
                .complete(
                    &outcome.payload.request.process_attempt_id,
                    &outcome.payload.request.request_sha256,
                    outcome.payload.result,
                )
                .await?;
        }
        cleanup::recover_finished(app, &entry.path()).await?;
    }
    Ok(())
}
async fn serve(path: &FsPath) -> Result<()> {
    let (configuration, signer, token) = config::load(path)?;
    // Binary replacement changes the binding and requires explicit operator setup.
    let executable = std::env::current_exe()?;
    ensure!(
        workspace::digest(&std::fs::read(executable)?)
            == configuration.binding.adapter_build_sha256,
        "Receiver executable no longer matches installed binding"
    );
    for name in [
        "attempts",
        "environments",
        "sandbox-sessions",
        "sandbox-operations",
    ] {
        config::private_directory(&configuration.data_directory.join(name))?;
    }
    let lock_path = configuration.data_directory.join("receiver.lock");
    if !lock_path.exists() {
        config::write_new(&lock_path, b"")?;
    }
    let lock = OpenOptions::new().read(true).write(true).open(&lock_path)?;
    lock.try_lock()
        .context("Another receiver owns this private data directory")?;
    let database = configuration.data_directory.join("runner.sqlite");
    if !database.exists() {
        config::write_new(&database, b"")?;
    }
    sqlx::any::install_default_drivers();
    let options = sqlx::any::AnyPoolOptions::new().max_connections(1);
    let pool = options
        .connect(&format!("sqlite://{}?mode=rw", database.display()))
        .await?;
    for statement in [
        "PRAGMA journal_mode=WAL",
        "PRAGMA synchronous=FULL",
        "PRAGMA busy_timeout=5000",
    ] {
        sqlx::query(statement).execute(&pool).await?;
    }
    let broker = Broker::open(pool, configuration.binding.clone(), signer.clone()).await?;
    let token_key = ring::hmac::Key::new(
        ring::hmac::HMAC_SHA256,
        b"localai-cowork/v2/transport-authority",
    );
    let token_tag = ring::hmac::sign(&token_key, token.as_bytes())
        .as_ref()
        .to_vec();
    let app = App(Arc::new(Inner {
        config: configuration.clone(),
        broker,
        signer,
        token_tag,
        token_key,
        serial: Arc::new(Semaphore::new(1)),
        stopping: AtomicBool::new(false),
    }));
    // Recovery may terminate exact owned resources, but never launches an action.
    recovery(&app).await?;
    cleanup::collect(&app).await?;
    let housekeeping = app.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if housekeeping.0.stopping.load(Ordering::Acquire) {
                break;
            }
            let Ok(_permit) = housekeeping.0.serial.clone().try_acquire_owned() else {
                continue;
            };
            if sandbox_lifecycle::maintain_idle(&housekeeping)
                .await
                .is_err()
            {
                eprintln!("Idle sandbox suspension remains unconfirmed; evidence retained");
            }
            if cleanup::collect(&housekeeping).await.is_err() {
                eprintln!("Retained cleanup remains unconfirmed; evidence retained");
            }
        }
    });
    let certs = pem(
        &config::private_read(&configuration.tls_certificate_file, 1024 * 1024)?,
        "CERTIFICATE",
    )?
    .into_iter()
    .map(CertificateDer::from)
    .collect();
    let keys = pem(
        &config::private_read(&configuration.tls_key_file, 65536)?,
        "PRIVATE KEY",
    )?;
    ensure!(keys.len() == 1, "Exactly one PKCS8 TLS key is required");
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            certs,
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(keys.into_iter().next().unwrap())),
        )?;
    let acceptor = TlsAcceptor::from(Arc::new(tls));
    let listener = TcpListener::bind(configuration.listen).await?;
    let router = Router::new()
        .route("/v2/fences", post(fence))
        .route("/v2/execution-leases", post(renew))
        .route("/v2/launches", post(launch))
        .route("/v2/receipts/{id}", get(receipt))
        .route("/v2/environments/observe", post(observe))
        .route("/v2/environments/install", post(install))
        .route(
            "/v2/sandboxes/lifecycle",
            post(sandbox_lifecycle::lifecycle),
        )
        .route(
            "/v2/sandboxes/operations/{id}",
            get(sandbox_lifecycle::receipt),
        )
        .route(
            "/v2/sandbox-previews/issue",
            post(sandbox_lifecycle::preview_issue),
        )
        .route(
            "/v2/sandbox-previews/validate",
            post(sandbox_lifecycle::preview_validate),
        )
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(app.clone());
    let connections = Arc::new(Semaphore::new(128));
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {_=&mut shutdown=>break,_=tokio::time::sleep(Duration::from_secs(1))=>{if app.0.stopping.load(Ordering::Acquire){break;}},accepted=listener.accept()=>{
            let (socket,_)=accepted?;let Ok(permit)=connections.clone().try_acquire_owned()else{continue;};let acceptor=acceptor.clone();let router=router.clone();
            tokio::spawn(async move{let _permit=permit;let Ok(Ok(socket))=tokio::time::timeout(Duration::from_secs(10),acceptor.accept(socket)).await else{return;};
                let service=hyper::service::service_fn(move|request|{let router=router.clone();async move{router.oneshot(request).await}});
                let _=tokio::time::timeout(Duration::from_secs(120),hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service)).await;
            });
        }}
    }
    app.0.stopping.store(true, Ordering::Release);
    let _quiescent = tokio::time::timeout(
        Duration::from_secs(45),
        app.0.serial.clone().acquire_owned(),
    )
    .await
    .context("Owned physical supervisor has not terminated")??;
    drop(lock);
    Ok(())
}
fn argument(values: &BTreeMap<String, String>, key: &str) -> Result<String> {
    values
        .get(key)
        .cloned()
        .with_context(|| format!("Missing --{key}"))
}
fn initialize(args: &[String]) -> Result<()> {
    ensure!(
        args.len().is_multiple_of(2),
        "Initialization requires --name value pairs"
    );
    let mut values = BTreeMap::new();
    for pair in args.chunks_exact(2) {
        ensure!(
            pair[0].starts_with("--")
                && values
                    .insert(pair[0][2..].to_owned(), pair[1].clone())
                    .is_none(),
            "Invalid initialization option"
        );
    }
    let data = PathBuf::from(argument(&values, "data")?);
    config::private_directory(&data)?;
    let workspace_state_directory = PathBuf::from(argument(&values, "workspace-state")?);
    config::ordinary(&workspace_state_directory, true)?;
    let tls_certificate_file = PathBuf::from(argument(&values, "tls-cert")?);
    let tls_key_file = PathBuf::from(argument(&values, "tls-key")?);
    config::private_read(&tls_certificate_file, 1024 * 1024)?;
    config::private_read(&tls_key_file, 65536)?;
    let probe_executable = PathBuf::from(argument(&values, "probe")?);
    config::ordinary(&probe_executable, false)?;
    let control_bytes = SigningKey::generate_pkcs8()?;
    let runner_bytes = SigningKey::generate_pkcs8()?;
    let control = SigningKey::from_pkcs8(&control_bytes)?;
    let runner = SigningKey::from_pkcs8(&runner_bytes)?;
    let binding = Binding {
        runner_id: uuid::Uuid::new_v4().to_string(),
        generation: 1,
        adapter_build_sha256: workspace::digest(&std::fs::read(std::env::current_exe()?)?),
        control_public_key: control.public_key(),
        runner_public_key: runner.public_key(),
    };
    let runner_key_file = data.join("runner-key.pk8");
    let control_key_file = data.join("control-key.pk8");
    let token_file = data.join("transport-token.txt");
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    config::write_new(&runner_key_file, &runner_bytes)?;
    config::write_new(&control_key_file, &control_bytes)?;
    config::write_new(&token_file, token.as_bytes())?;
    let configuration = Configuration {
        format: 1,
        listen: argument(&values, "listen")?.parse()?,
        binding: binding.clone(),
        data_directory: data.clone(),
        workspace_state_directory,
        token_file: token_file.clone(),
        runner_key_file,
        tls_certificate_file: tls_certificate_file.clone(),
        tls_key_file,
        probe_executable,
        docker_executable: values.get("docker").map(PathBuf::from),
        local_container_profile: values
            .get("local-container-profile")
            .is_some_and(|v| v == "true"),
        local_engine_id: values.get("local-engine-id").cloned(),
        agent_sandbox: values
            .get("agent-sandbox-config")
            .map(|path| -> Result<cowork_agent_sandbox::Configuration> {
                Ok(serde_json::from_slice(&config::private_read(
                    FsPath::new(path),
                    65536,
                )?)?)
            })
            .transpose()?,
        windows_bundles: BTreeMap::new(),
        retention_seconds: 7 * 24 * 60 * 60,
        max_retained_attempts: 256,
    };
    let public_origin = argument(&values, "public-origin")?;
    ensure!(
        public_origin.starts_with("https://") && !public_origin.contains(['\n', '\r']),
        "An HTTPS public origin is required"
    );
    config::write_new(
        &data.join("runner.json"),
        &serde_json::to_vec_pretty(&configuration)?,
    )?;
    config::write_new(
        &data.join("daemon-runner.json"),
        &serde_json::to_vec_pretty(
            &serde_json::json!({"format":1,"endpoint":public_origin,"binding":binding,"control_key_file":control_key_file,"transport_token_file":token_file,"ca_certificate_file":tls_certificate_file,"client_identity_file":null}),
        )?,
    )?;
    println!(
        "Private receiver configuration: {}\nDaemon COWORK_PROCESS_RUNNER_CONFIG: {}",
        data.join("runner.json").display(),
        data.join("daemon-runner.json").display()
    );
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") if args.len() == 1 => {
            println!(
                "{}",
                serde_json::json!({"application_version":env!("CARGO_PKG_VERSION"),"runner_protocol_version":1,"binary":"cowork-process-runner","os":std::env::consts::OS,"arch":std::env::consts::ARCH})
            );
            Ok(())
        }
        Some("init") => initialize(&args[1..]),
        Some("serve") if args.len() == 2 => serve(FsPath::new(&args[1])).await,
        #[cfg(windows)]
        Some("orphan") => windows_orphan::command(&args[1..]),
        _ => anyhow::bail!(
            "Usage: cowork-process-runner init --data ABS --workspace-state ABS --tls-cert ABS --tls-key ABS --probe ABS --listen IP:PORT --public-origin https://HOST:PORT [--docker ABS]; or serve ABS_CONFIG; Windows: orphan (inspect/confirm/receipt syntax)"
        ),
    }
}

#[cfg(test)]
mod cleanup_authority_tests {
    use super::*;

    #[tokio::test]
    async fn signed_history_allows_cleanup_without_new_image_admission() {
        sqlx::any::install_default_drivers();
        let root = tempfile::tempdir().unwrap();
        let control = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
        let signer = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
        let binding = Binding {
            runner_id: "fixture".into(),
            generation: 1,
            adapter_build_sha256: "a".repeat(64),
            control_public_key: control.public_key(),
            runner_public_key: signer.public_key(),
        };
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let broker = Broker::open(pool, binding.clone(), signer.clone())
            .await
            .unwrap();
        let config = Configuration {
            format: 1,
            listen: "127.0.0.1:8443".parse().unwrap(),
            binding,
            data_directory: root.path().to_path_buf(),
            workspace_state_directory: root.path().to_path_buf(),
            token_file: root.path().join("token"),
            runner_key_file: root.path().join("key"),
            tls_certificate_file: root.path().join("cert"),
            tls_key_file: root.path().join("tls-key"),
            probe_executable: root.path().join("probe"),
            docker_executable: None,
            local_container_profile: false,
            local_engine_id: None,
            agent_sandbox: None,
            windows_bundles: BTreeMap::new(),
            retention_seconds: 900,
            max_retained_attempts: 2,
        };
        config::private_directory(&root.path().join("environments")).unwrap();
        let definition = serde_json::from_value(serde_json::json!({"format_version":2,"artifact":{"kind":"linux_kata_image","reference":format!("registry.example/runtime@sha256:{}","b".repeat(64))},"programs":{"python":"usr/bin/python3"},"packages":[],"limits":{"cpu_millis":500,"memory_bytes":268435456,"processes":64,"scratch_bytes":268435456},"network":{"kind":"denied"}})).unwrap();
        let install = EnvironmentInstall {
            environment_id: "environment".into(),
            version_id: "v1".into(),
            definition,
        };
        let qualification = EnvironmentQualification {
            runner_id: config.binding.runner_id.clone(),
            runner_generation: 1,
            adapter_build_sha256: config.binding.adapter_build_sha256.clone(),
            artifact_sha256: "b".repeat(64),
            definition_sha256: canonical_hash(&install.definition),
            report_sha256: "c".repeat(64),
            capabilities: install.definition.required_capabilities(),
            verified_at: 1,
        };
        let reference = ExecutionEnvironmentRef {
            environment_id: install.environment_id.clone(),
            version_id: install.version_id.clone(),
            definition_sha256: canonical_hash(&install.definition),
            qualification_sha256: canonical_hash(&qualification),
        };
        let record = Installed {
            request: control.sign("environment_install", install).unwrap(),
            qualification: signer
                .sign("environment_qualification", qualification)
                .unwrap(),
        };
        config::durable_json(
            &environment_path(&config, &reference.environment_id, &reference.version_id),
            &record,
        )
        .unwrap();
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"fixture");
        let app = App(Arc::new(Inner {
            config,
            broker,
            signer,
            token_tag: vec![],
            token_key: key,
            serial: Arc::new(Semaphore::new(1)),
            stopping: AtomicBool::new(false),
        }));
        assert!(installed_record(&app, &reference, false).is_ok());
        assert!(
            installed(&app, &reference).is_err(),
            "Fresh launch must require currently available operator runtime"
        );
        let mut forged = reference;
        forged.qualification_sha256 = "f".repeat(64);
        assert!(
            installed_record(&app, &forged, false).is_err(),
            "Cleanup must retain exact signed historical identity"
        );
    }
}
