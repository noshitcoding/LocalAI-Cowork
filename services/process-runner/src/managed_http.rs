//! Private HTTPS application routes. TLS and operator bootstrap are owned by
//! the dedicated entry point. No endpoint accepts a host path or OCI spec.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_domain::canonical_hash;
use cowork_domain::environments::{EnvironmentObservation, EnvironmentQualification};
use cowork_managed_runtime::containerd::{
    backend::{
        BeginInput, ManagedCompletion, ManagedOutputStatus, ManagedReceiver, ManagedTransfer,
    },
    bootstrap::InstalledManagedProfile,
};
use cowork_runner_protocol::*;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};

const MAX_READY: usize = 128;
const MAX_RESULTS: usize = 256;
mod delivery;
mod result_context;
pub(super) struct Persistence {
    pub diagnostics: super::managed_diagnostics::Diagnostics,
    pub journal: super::managed_delivery_journal::Journal,
}
#[derive(Clone)]
pub(super) struct Service(Arc<Inner>);
struct Inner {
    backend: Option<ManagedReceiver>,
    broker: Broker,
    signer: SigningKey,
    profile: InstalledManagedProfile,
    token_key: ring::hmac::Key,
    token_tag: Vec<u8>,
    io: Arc<Semaphore>,
    ready: Mutex<BTreeMap<String, Ready>>,
    input: Mutex<Option<Input>>,
    started: Mutex<BTreeMap<String, Started>>,
    results: Mutex<BTreeMap<String, Arc<Published>>>,
    failures: Mutex<BTreeMap<String, &'static str>>,
    retention_millis: i64,
    diagnostics: super::managed_diagnostics::Diagnostics,
    journal: super::managed_delivery_journal::Journal,
    delivery_admission: Mutex<()>,
    retirements: std::sync::Mutex<BTreeMap<String, Arc<std::sync::Mutex<delivery::Custody>>>>,
}
struct Ready {
    signed: Signed<ManagedReadiness>,
    deadline: Instant,
}
struct Input {
    begin: ManagedBeginRequest,
    ticket: Signed<ManagedInputTicket>,
    transfer: Box<ManagedTransfer>,
    deadline: Instant,
}
#[derive(Clone)]
struct Started {
    ticket: Signed<ManagedInputTicket>,
    attempt: String,
}
struct Published {
    retiring: AtomicBool,
    launch: Signed<LaunchRequest>,
    grant: Signed<ManagedInputGrant>,
    completion: ManagedCompletion,
    attestation: Option<Signed<ManagedOutputAttestation>>,
    retention: Option<OutputRetention>,
    boot_retention: Option<cowork_managed_runtime::containerd::backend::ManagedOutputRetention>,
}
struct OutputRetention {
    retained_until: i64,
    deadline: Instant,
}
impl OutputRetention {
    fn new(
        original_deadline: Instant,
        retained_until: i64,
        observed_at: i64,
        observed_instant: Instant,
    ) -> Self {
        let remaining = retained_until
            .checked_sub(observed_at)
            .filter(|v| *v > 0)
            .unwrap_or(0) as u64;
        let deadline = observed_instant
            .checked_add(Duration::from_millis(remaining))
            .unwrap_or(observed_instant);
        Self {
            retained_until,
            deadline: original_deadline.min(deadline),
        }
    }
    fn current(&self, wall: i64, instant: Instant) -> bool {
        wall < self.retained_until && instant < self.deadline
    }
}
type Answer<T> = std::result::Result<T, ApiError>;
pub(super) struct ApiError(pub(super) StatusCode, pub(super) &'static str);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"error":self.1}))).into_response()
    }
}
fn denied() -> ApiError {
    ApiError(StatusCode::FORBIDDEN, "managed_authority_denied")
}
fn broker_denied(
    service: &Service,
    phase: &'static str,
    attempt: &str,
    error: &cowork_runner_protocol::Error,
) -> ApiError {
    let _ = service.0.diagnostics.record(
        phase,
        attempt,
        error,
        serde_json::json!(service.0.broker.last_clock_rollback_observation()),
    );
    denied()
}
fn uncertain() -> ApiError {
    ApiError(StatusCode::CONFLICT, "managed_execution_unconfirmed")
}
/// Only after original live provenance has been resolved by the native owner.
/// Reobserve original storage after every await before exposing non-acceptance.
pub(super) fn delivery_window_after_origin(
    journal: &super::managed_delivery_journal::Journal,
    request: &Signed<ManagedResultDeliveryRequest>,
    observed: i64,
) -> Answer<Option<Signed<ManagedResultDeliveryReceipt>>> {
    if let Some(original) = journal
        .existing(request)
        .map_err(|_| delivery::unconfirmed())?
    {
        return Ok(Some(original));
    }
    if observed >= request.payload.accept_before {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "managed_result_delivery_confirmation_expired",
        ));
    }
    Ok(None)
}
fn now() -> Answer<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| uncertain())?
        .as_millis()
        .try_into()
        .map_err(|_| uncertain())
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}

impl Service {
    pub(super) fn new(
        backend: Option<ManagedReceiver>,
        broker: Broker,
        signer: SigningKey,
        profile: InstalledManagedProfile,
        token: &str,
        retention_millis: i64,
        persistence: Persistence,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            token.len() >= 48 && !token.chars().any(char::is_whitespace),
            "Invalid runtime transport credential"
        );
        anyhow::ensure!(
            retention_millis > 0 && retention_millis <= MANAGED_RESULT_RETENTION_MILLIS,
            "Invalid result retention"
        );
        anyhow::ensure!(
            signer.public_key() == profile.binding.runner_public_key
                && broker.is_bound_to(&profile.binding),
            "Runtime identity mismatch"
        );
        if let Some(active) = &backend {
            active.ready(&profile.reference, &profile.target)?;
        }
        let token_key =
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"cowork-managed-transport-v1");
        let token_tag = ring::hmac::sign(&token_key, token.as_bytes())
            .as_ref()
            .to_vec();
        Ok(Self(Arc::new(Inner {
            backend,
            broker,
            signer,
            profile,
            token_key,
            token_tag,
            io: Arc::new(Semaphore::new(2)),
            ready: Mutex::new(BTreeMap::new()),
            input: Mutex::new(None),
            started: Mutex::new(BTreeMap::new()),
            results: Mutex::new(BTreeMap::new()),
            failures: Mutex::new(BTreeMap::new()),
            retention_millis,
            diagnostics: persistence.diagnostics,
            journal: persistence.journal,
            delivery_admission: Mutex::new(()),
            retirements: std::sync::Mutex::new(BTreeMap::new()),
        })))
    }
    pub(super) fn router(&self) -> Router {
        let history = Router::new()
            .route("/v2/receipts/{id}", get(receipt))
            .route(
                "/v2/managed/results/{id}/delivery/history",
                post(delivery::history),
            )
            .route(
                "/v2/managed/results/{id}/retirement",
                post(delivery::status),
            );
        let routes = if self.0.backend.is_none() {
            history.route("/v2/managed/results/{id}/delivery", post(delivery::history))
        } else {
            history
                .route("/v2/managed/results/{id}/delivery", post(delivery::confirm))
                .route("/v2/environments/install", post(installed_environment))
                .route("/v2/environments/observe", post(observe_environment))
                .route("/v2/managed/ready", post(ready))
                .route("/v2/managed/inputs", post(begin))
                .route("/v2/managed/inputs/{id}/missing", post(missing))
                .route("/v2/managed/inputs/{id}/chunks", post(chunk))
                .route("/v2/managed/inputs/{id}/start", post(start))
                .route("/v2/managed/results/{id}/read", post(read_result))
                .route(
                    "/v2/managed/results/{id}/context",
                    post(result_context::observe),
                )
                .route("/v2/fences", post(fence))
                .route("/v2/execution-leases", post(renew))
                .route("/v2/launches", post(upgrade))
        };
        routes
            .layer(DefaultBodyLimit::max(MANAGED_CHUNK_BODY_BYTES))
            .route_layer(middleware::from_fn_with_state(self.clone(), authenticate))
            .with_state(self.clone())
    }
    fn backend(&self) -> Answer<&ManagedReceiver> {
        self.0.backend.as_ref().ok_or(ApiError(
            StatusCode::CONFLICT,
            "managed_runtime_recovery_only",
        ))
    }
    fn ticket(&self, input: &Input, received: &Signed<ManagedInputTicket>, id: &str) -> Answer<()> {
        received
            .verify(
                "managed_input_ticket",
                &self.0.profile.binding.runner_public_key,
            )
            .map_err(|_| denied())?;
        received
            .payload
            .validate(&input.begin, now()?)
            .map_err(|_| denied())?;
        if input.deadline <= Instant::now()
            || received.payload.input_id != id
            || received.payload.runtime_instance_sha256 != self.0.broker.instance_fingerprint()
            || canonical_hash(received) != canonical_hash(&input.ticket)
        {
            return Err(denied());
        }
        Ok(())
    }
    async fn publish(
        &self,
        begin: ManagedBeginRequest,
        execution: cowork_managed_runtime::containerd::backend::ManagedExecution,
    ) {
        let attempt = begin.launch.payload.process_attempt_id.clone();
        let completed = match execution.wait().await {
            Ok(value) => value,
            Err(error) => {
                let _ = self.0.diagnostics.record(
                    "managed_execution_wait",
                    &attempt,
                    &error,
                    serde_json::json!(self.0.broker.last_clock_rollback_observation()),
                );
                self.0
                    .failures
                    .lock()
                    .await
                    .insert(attempt, "managed_execution_unconfirmed");
                return;
            }
        };
        let attestation = match &completed.output {
            ManagedOutputStatus::Rejected(_) => None,
            ManagedOutputStatus::Ready(snapshot) => {
                let make = || -> anyhow::Result<Signed<ManagedOutputAttestation>> {
                    let manifest = snapshot.manifest()?;
                    let input = &begin.grant.payload;
                    let produced_at = completed.receipt.payload.updated_at;
                    let retained_until = produced_at
                        .checked_add(self.0.retention_millis)
                        .ok_or_else(|| anyhow::anyhow!("Retention overflow"))?;
                    let signed = self.0.signer.sign(
                        "managed_output",
                        ManagedOutputAttestation {
                            format_version: 1,
                            runtime_instance_sha256: self.0.broker.instance_fingerprint(),
                            execution_plan_id: input.execution_plan_id.clone(),
                            execution_plan_sha256: input.execution_plan_sha256.clone(),
                            process_attempt_id: input.process_attempt_id.clone(),
                            run_id: input.run_id.clone(),
                            launch_envelope_sha256: envelope_hash(&begin.launch),
                            input_manifest_sha256: input.input_manifest_sha256.clone(),
                            output_manifest_sha256: manifest.sha256,
                            execution_receipt_sha256: canonical_hash(&completed.receipt),
                            physical_record_sha256: completed
                                .physical
                                .physical_record_sha256
                                .clone(),
                            runner: input.runner.clone(),
                            target: input.target.clone(),
                            produced_at,
                            retained_until,
                        },
                    )?;
                    snapshot.retain_attestation(&begin.launch, &completed.receipt, &signed)?;
                    Ok(signed)
                };
                make().ok()
            }
        };
        let retention = attestation.as_ref().and_then(|attestation| {
            Some(OutputRetention::new(
                completed.output_retention_deadline(Duration::from_millis(
                    self.0.retention_millis as u64,
                ))?,
                attestation.payload.retained_until,
                now().ok()?,
                Instant::now(),
            ))
        });
        let boot_retention =
            completed.output_retention_fence(Duration::from_millis(self.0.retention_millis as u64));
        self.0.results.lock().await.insert(
            attempt,
            Arc::new(Published {
                retiring: AtomicBool::new(false),
                launch: begin.launch,
                grant: begin.grant,
                completion: completed,
                attestation,
                retention,
                boot_retention,
            }),
        );
    }
}
async fn authenticate(State(service): State<Service>, request: Request, next: Next) -> Response {
    let token = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    if token.is_none_or(|token| {
        ring::hmac::verify(&service.0.token_key, token.as_bytes(), &service.0.token_tag).is_err()
    }) {
        return ApiError(
            StatusCode::UNAUTHORIZED,
            "managed_transport_authority_required",
        )
        .into_response();
    }
    if request.uri().path() == "/v2/launches" {
        return ApiError(
            StatusCode::CONFLICT,
            "managed_execution_plan_upgrade_required",
        )
        .into_response();
    }
    let Ok(_permit) = service.0.io.clone().try_acquire_owned() else {
        return ApiError(StatusCode::SERVICE_UNAVAILABLE, "managed_receiver_busy").into_response();
    };
    // Authentication and bounded concurrent body ownership precede extraction.
    let maximum = if request.uri().path().ends_with("/chunks") {
        MANAGED_CHUNK_BODY_BYTES
    } else if matches!(
        request.uri().path(),
        "/v2/managed/inputs" | "/v2/environments/install"
    ) {
        MANAGED_BEGIN_BODY_BYTES
    } else if request.uri().path().ends_with("/delivery")
        || request.uri().path().ends_with("/delivery/history")
        || request.uri().path().ends_with("/retirement")
    {
        MANAGED_RESULT_DELIVERY_BYTES
    } else {
        65536
    };
    let (parts, body) = request.into_parts();
    let bytes =
        match tokio::time::timeout(Duration::from_secs(15), axum::body::to_bytes(body, maximum))
            .await
        {
            Ok(Ok(bytes)) => bytes,
            _ => {
                return ApiError(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "managed_body_limit_or_timeout",
                )
                .into_response();
            }
        };
    next.run(Request::from_parts(parts, axum::body::Body::from(bytes)))
        .await
}
async fn installed_environment(
    State(service): State<Service>,
    Json(request): Json<Signed<EnvironmentInstall>>,
) -> Answer<Json<Signed<EnvironmentQualification>>> {
    request
        .verify(
            "environment_install",
            &service.0.profile.binding.control_public_key,
        )
        .map_err(|_| denied())?;
    let profile = &service.0.profile;
    if request.payload.environment_id != profile.reference.environment_id
        || request.payload.version_id != profile.reference.version_id
        || request.payload.definition != profile.definition
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "managed_environment_package_not_installed",
        ));
    }
    // This compatibility route acknowledges only the exact already-installed,
    // externally qualified operator package. It creates no qualification,
    // installation, execution row or alternative native-host adapter.
    service
        .backend()?
        .ready(&profile.reference, &profile.target)
        .map_err(|_| uncertain())?;
    Ok(Json(profile.qualification.clone()))
}
async fn observe_environment(
    State(service): State<Service>,
    Json(reference): Json<cowork_contracts::ExecutionEnvironmentRef>,
) -> Answer<Json<Signed<EnvironmentObservation>>> {
    let profile = &service.0.profile;
    service
        .backend()?
        .ready(&reference, &profile.target)
        .map_err(|error| {
            let _ = service.0.diagnostics.record(
                "managed_environment_observe",
                &reference.environment_id,
                &error,
                serde_json::json!(service.0.broker.last_clock_rollback_observation()),
            );
            uncertain()
        })?;
    let q = &profile.qualification.payload;
    let observation = EnvironmentObservation {
        runner_id: q.runner_id.clone(),
        runner_generation: q.runner_generation,
        adapter_build_sha256: q.adapter_build_sha256.clone(),
        artifact_sha256: q.artifact_sha256.clone(),
        definition_sha256: q.definition_sha256.clone(),
        capabilities: q.capabilities.clone(),
        observed_at: now()?,
    };
    Ok(Json(
        service
            .0
            .signer
            .sign("environment_observation", observation)
            .map_err(|_| uncertain())?,
    ))
}
async fn ready(
    State(service): State<Service>,
    Json(request): Json<ManagedReadyRequest>,
) -> Answer<Json<Signed<ManagedReadiness>>> {
    service
        .backend()?
        .ready(&request.environment, &request.target)
        .map_err(|_| uncertain())?;
    let mut cache = service.0.ready.lock().await;
    cache.retain(|_, value| Instant::now() < value.deadline);
    if let Some(previous) = cache.get(&request.request_id) {
        previous
            .signed
            .payload
            .validate(&request, &service.0.profile.binding, now()?)
            .map_err(|_| denied())?;
        return Ok(Json(previous.signed.clone()));
    }
    if cache.len() >= MAX_READY {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "managed_readiness_busy",
        ));
    }
    let observed_at = now()?;
    let signed = service
        .0
        .signer
        .sign(
            "managed_readiness",
            ManagedReadiness {
                format_version: 1,
                request_id: request.request_id.clone(),
                request_sha256: canonical_hash(&request),
                runner: cowork_contracts::ExecutionRunnerIdentity {
                    runner_id: service.0.profile.binding.runner_id.clone(),
                    generation: service.0.profile.binding.generation,
                    adapter_build_sha256: service.0.profile.binding.adapter_build_sha256.clone(),
                    control_public_key: service.0.profile.binding.control_public_key.clone(),
                    runner_public_key: service.0.profile.binding.runner_public_key.clone(),
                },
                environment: request.environment.clone(),
                target: request.target.clone(),
                runtime_instance_sha256: service.0.broker.instance_fingerprint(),
                observed_at,
                expires_at: observed_at
                    .checked_add(MANAGED_READINESS_MILLIS)
                    .ok_or_else(uncertain)?,
            },
        )
        .map_err(|_| uncertain())?;
    signed
        .payload
        .validate(&request, &service.0.profile.binding, observed_at)
        .map_err(|_| denied())?;
    cache.insert(
        request.request_id,
        Ready {
            signed: signed.clone(),
            deadline: Instant::now() + Duration::from_millis(MANAGED_READINESS_MILLIS as u64),
        },
    );
    Ok(Json(signed))
}
async fn begin(
    State(service): State<Service>,
    Json(request): Json<ManagedBeginRequest>,
) -> Answer<Response> {
    if request.format_version != 1 {
        return Err(denied());
    }
    request
        .readiness
        .verify(
            "managed_readiness",
            &service.0.profile.binding.runner_public_key,
        )
        .map_err(|_| denied())?;
    let expected = ManagedReadyRequest {
        format_version: 1,
        request_id: request.readiness.payload.request_id.clone(),
        environment: request.launch.payload.environment.clone(),
        target: request.grant.payload.target.clone(),
    };
    request
        .readiness
        .payload
        .validate(&expected, &service.0.profile.binding, now()?)
        .map_err(|_| denied())?;
    if request.readiness.payload.runtime_instance_sha256 != service.0.broker.instance_fingerprint()
    {
        return Err(denied());
    }
    let mut slot = service.0.input.lock().await;
    if slot
        .as_ref()
        .is_some_and(|input| Instant::now() >= input.deadline)
    {
        slot.take();
    }
    if let Some(input) = slot.as_ref() {
        if canonical_hash(&input.begin) != canonical_hash(&request) {
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "managed_receiver_busy",
            ));
        }
        service.ticket(input, &input.ticket, &input.ticket.payload.input_id)?;
        return Ok((
            StatusCode::OK,
            Json(ManagedBeginResponse::Input {
                ticket: input.ticket.clone(),
            }),
        )
            .into_response());
    }
    if service.0.started.lock().await.len() >= MAX_RESULTS {
        return Err(uncertain());
    }
    let prepared = service
        .0
        .ready
        .lock()
        .await
        .remove(&expected.request_id)
        .ok_or_else(denied)?;
    if prepared.deadline <= Instant::now()
        || canonical_hash(&prepared.signed) != canonical_hash(&request.readiness)
    {
        return Err(denied());
    }
    let expires = request
        .launch
        .payload
        .expires_at
        .min(request.grant.payload.expires_at);
    let started = Instant::now();
    let remaining = expires
        .checked_sub(now()?)
        .filter(|n| *n > 0)
        .ok_or_else(denied)?;
    service
        .0
        .journal
        .reserve(&request, &service.0.broker.instance_fingerprint(), now()?)
        .map_err(|_| uncertain())?;
    match service
        .backend()?
        .begin(
            request.launch.clone(),
            request.grant.clone(),
            request.manifest.clone(),
        )
        .await
        .map_err(|error| {
            let _ = service.0.diagnostics.record(
                "managed_input_begin",
                &request.launch.payload.process_attempt_id,
                &error,
                serde_json::json!(service.0.broker.last_clock_rollback_observation()),
            );
            uncertain()
        })? {
        BeginInput::Existing(receipt) => Ok((
            StatusCode::OK,
            Json(ManagedBeginResponse::Execution { receipt: *receipt }),
        )
            .into_response()),
        BeginInput::Transfer(transfer) => {
            let ticket = service
                .0
                .signer
                .sign(
                    "managed_input_ticket",
                    ManagedInputTicket {
                        format_version: 1,
                        input_id: uuid::Uuid::new_v4().to_string(),
                        launch_envelope_sha256: envelope_hash(&request.launch),
                        grant_envelope_sha256: canonical_hash(&request.grant),
                        manifest_sha256: request.manifest.sha256.clone(),
                        readiness_sha256: canonical_hash(&request.readiness),
                        runtime_instance_sha256: service.0.broker.instance_fingerprint(),
                        expires_at: expires,
                    },
                )
                .map_err(|_| uncertain())?;
            ticket
                .payload
                .validate(&request, now()?)
                .map_err(|_| denied())?;
            *slot = Some(Input {
                begin: request,
                ticket: ticket.clone(),
                transfer,
                deadline: started
                    + Duration::from_millis(remaining.try_into().map_err(|_| denied())?),
            });
            Ok((
                StatusCode::CREATED,
                Json(ManagedBeginResponse::Input { ticket }),
            )
                .into_response())
        }
    }
}
async fn missing(
    State(service): State<Service>,
    Path(id): Path<String>,
    Json(request): Json<ManagedMissingRequest>,
) -> Answer<Json<ManagedMissingResponse>> {
    let slot = service.0.input.lock().await;
    let input = slot.as_ref().ok_or_else(|| {
        let _ = service.0.diagnostics.record(
            "managed_input_missing_absent",
            &id,
            &"original_input_slot_absent",
            serde_json::json!(service.0.broker.last_clock_rollback_observation()),
        );
        uncertain()
    })?;
    service.ticket(input, &request.ticket, &id)?;
    let missing = input.transfer.missing_chunks().await.map_err(|error| {
        let _ = service.0.diagnostics.record(
            "managed_input_missing",
            &input.begin.launch.payload.process_attempt_id,
            &error,
            serde_json::json!(service.0.broker.last_clock_rollback_observation()),
        );
        uncertain()
    })?;
    let response = ManagedMissingResponse {
        ticket_sha256: canonical_hash(&input.ticket),
        missing,
    };
    response
        .validate(&input.ticket, &input.begin.manifest)
        .map_err(|error| {
            let _ = service.0.diagnostics.record(
                "managed_input_missing_response",
                &input.begin.launch.payload.process_attempt_id,
                &error,
                serde_json::json!(service.0.broker.last_clock_rollback_observation()),
            );
            uncertain()
        })?;
    service.ticket(input, &request.ticket, &id)?;
    Ok(Json(response))
}
async fn chunk(
    State(service): State<Service>,
    Path(id): Path<String>,
    Json(request): Json<ManagedChunkRequest>,
) -> Answer<Json<ManagedChunkAck>> {
    let slot = service.0.input.lock().await;
    let input = slot.as_ref().ok_or_else(uncertain)?;
    service.ticket(input, &request.ticket, &id)?;
    let bytes = STANDARD
        .decode(&request.bytes_base64)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "managed_chunk_encoding_invalid"))?;
    if bytes.len() as u64 > cowork_domain::workspace_transfer::CHUNK_BYTES {
        return Err(denied());
    }
    let receipt = input
        .transfer
        .accept_chunk(&request.chunk, &bytes)
        .await
        .map_err(|error| {
            let _ = service.0.diagnostics.record(
                "managed_input_chunk",
                &input.begin.launch.payload.process_attempt_id,
                &error,
                serde_json::json!(service.0.broker.last_clock_rollback_observation()),
            );
            uncertain()
        })?;
    service.ticket(input, &request.ticket, &id)?;
    Ok(Json(ManagedChunkAck {
        receipt_sha256: receipt.receipt_sha256,
        path: receipt.path,
        offset: receipt.offset,
        size: receipt.size,
        sha256: receipt.sha256,
    }))
}
async fn start(
    State(service): State<Service>,
    Path(id): Path<String>,
    Json(request): Json<ManagedStartRequest>,
) -> Answer<Response> {
    let mut slot = service.0.input.lock().await;
    // An old ticket can read only its own original start receipt. It never
    // re-enters transfer/seal/dispatch and cannot acquire fresh execution rights.
    let previous = service.0.started.lock().await.get(&id).cloned();
    let attempt = if let Some(previous) = previous {
        request
            .ticket
            .verify(
                "managed_input_ticket",
                &service.0.profile.binding.runner_public_key,
            )
            .map_err(|_| denied())?;
        if canonical_hash(&previous.ticket) != canonical_hash(&request.ticket)
            || request.ticket.payload.runtime_instance_sha256
                != service.0.broker.instance_fingerprint()
        {
            return Err(denied());
        }
        previous.attempt
    } else {
        service.ticket(slot.as_ref().ok_or_else(uncertain)?, &request.ticket, &id)?;
        let input = slot.take().ok_or_else(uncertain)?;
        let attempt = input.begin.launch.payload.process_attempt_id.clone();
        service.0.started.lock().await.insert(
            id,
            Started {
                ticket: input.ticket.clone(),
                attempt: attempt.clone(),
            },
        );
        // The server owns sealing as well as execution. A disconnected HTTP
        // request must not strand an irreversible, successfully sealed input.
        let publisher = service.clone();
        tokio::spawn(async move {
            match (*input.transfer).seal_and_start().await {
                Ok(execution) => publisher.publish(input.begin, execution).await,
                Err(error) => {
                    let _ = publisher.0.diagnostics.record(
                        "managed_seal_and_start",
                        &input.begin.launch.payload.process_attempt_id,
                        &error,
                        serde_json::json!(publisher.0.broker.last_clock_rollback_observation()),
                    );
                    publisher.0.failures.lock().await.insert(
                        input.begin.launch.payload.process_attempt_id,
                        "managed_execution_unconfirmed",
                    );
                }
            }
        });
        attempt
    };
    drop(slot);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match service.0.broker.receipt(&attempt).await {
            Ok(receipt) => return Ok((StatusCode::ACCEPTED, Json(receipt)).into_response()),
            Err(cowork_runner_protocol::Error::NotFound) => {}
            Err(_) => return Err(uncertain()),
        }
        if Instant::now() >= deadline || service.0.failures.lock().await.contains_key(&attempt) {
            return Err(uncertain());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
async fn receipt(State(service): State<Service>, Path(id): Path<String>) -> Answer<Response> {
    if !identifier(&id) {
        return Err(denied());
    }
    let receipt = service
        .0
        .broker
        .receipt(&id)
        .await
        .map_err(|_| uncertain())?;
    let status = if matches!(receipt.payload.state, ExecutionState::Completed { .. }) {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    Ok((status, Json(receipt)).into_response())
}
async fn fence(
    State(service): State<Service>,
    Json(request): Json<Signed<FenceRequest>>,
) -> Answer<StatusCode> {
    service
        .0
        .broker
        .fence(request)
        .await
        .map_err(|_| denied())?;
    Ok(StatusCode::NO_CONTENT)
}
async fn renew(
    State(_service): State<Service>,
    Json(_request): Json<Signed<ExecutionLeaseRenewal>>,
) -> Answer<StatusCode> {
    // The independent physical helper currently enforces the original lease.
    // Do not acknowledge a logical extension the physical fence cannot honor.
    Err(ApiError(
        StatusCode::CONFLICT,
        "managed_lease_extension_unavailable",
    ))
}
async fn upgrade() -> ApiError {
    ApiError(
        StatusCode::CONFLICT,
        "managed_execution_plan_upgrade_required",
    )
}
async fn read_result(
    State(service): State<Service>,
    Path(id): Path<String>,
    Json(signed): Json<Signed<ManagedResultReadRequest>>,
) -> Answer<Response> {
    if !identifier(&id) || signed.payload.process_attempt_id != id {
        return Err(denied());
    }
    let permit = service
        .0
        .broker
        .authorize_managed_result_read(signed, &service.0.profile.target)
        .await
        .map_err(|error| broker_denied(&service, "managed_result_read_authorize", &id, &error))?;
    let published = service
        .0
        .results
        .lock()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(uncertain)?;
    let request = permit.request();
    if published.retiring.load(Ordering::Acquire) {
        return Err(delivery::unconfirmed());
    }
    let grant = &published.grant.payload;
    if request.execution_plan_id != grant.execution_plan_id
        || request.execution_plan_sha256 != grant.execution_plan_sha256
        || request.run_id != grant.run_id
        || request.launch_envelope_sha256 != envelope_hash(&published.launch)
        || request.execution_receipt_sha256 != canonical_hash(&published.completion.receipt)
    {
        return Err(denied());
    }
    let snapshot = match &published.completion.output {
        ManagedOutputStatus::Ready(snapshot) => snapshot,
        ManagedOutputStatus::Rejected(rejection) => {
            let reason = match rejection.reason.as_str() {
                "result_symlink_unsupported" => "result_symlink_unsupported",
                "result_special_file_unsupported" => "result_special_file_unsupported",
                "result_portable_entry_limit_exceeded" => "result_portable_entry_limit_exceeded",
                "result_portable_path_unsupported" => "result_portable_path_unsupported",
                "result_export_io_failed" => "result_export_io_failed",
                _ => "result_snapshot_unavailable_or_changed",
            };
            return Err(ApiError(StatusCode::CONFLICT, reason));
        }
    };
    let Some(attestation) = &published.attestation else {
        return Err(ApiError(StatusCode::CONFLICT, "managed_result_unavailable"));
    };
    let retention = published.retention.as_ref().ok_or_else(uncertain)?;
    if !retention.current(now()?, Instant::now())
        || !published
            .boot_retention
            .as_ref()
            .is_some_and(|fence| fence.current())
    {
        return Err(ApiError(
            StatusCode::GONE,
            "managed_result_retention_expired",
        ));
    }
    let response = match &permit.request().operation {
        ManagedResultReadOperation::Manifest => Json(ManagedResultManifest {
            attestation: attestation.clone(),
            manifest: snapshot.manifest().map_err(|_| uncertain())?,
        })
        .into_response(),
        ManagedResultReadOperation::Chunk {
            attestation_sha256,
            manifest_sha256,
            chunk,
        } => {
            if *attestation_sha256 != canonical_hash(attestation)
                || *manifest_sha256 != attestation.payload.output_manifest_sha256
            {
                return Err(denied());
            }
            let bytes = snapshot.chunk(chunk).map_err(|_| uncertain())?;
            (
                [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
                bytes,
            )
                .into_response()
        }
    };
    service
        .0
        .broker
        .consume_managed_result_read(permit, &service.0.profile.target)
        .await
        .map_err(|error| broker_denied(&service, "managed_result_read_consume", &id, &error))?;
    if published.retiring.load(Ordering::Acquire) {
        return Err(delivery::unconfirmed());
    }
    if !retention.current(now()?, Instant::now())
        || !published
            .boot_retention
            .as_ref()
            .is_some_and(|fence| fence.current())
    {
        return Err(ApiError(
            StatusCode::GONE,
            "managed_result_retention_expired",
        ));
    }
    Ok(response)
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    #[test]
    fn original_output_retention_survives_slow_export_and_backward_utc_without_extension() {
        let origin = Instant::now();
        // Export has already spent seven of the original eight seconds. UTC
        // moves backwards yet remains compatible with the separate read grant.
        let retention = OutputRetention::new(
            origin + Duration::from_secs(8),
            18000,
            11000,
            origin + Duration::from_secs(7),
        );
        assert!(retention.current(11001, origin + Duration::from_millis(7999)));
        assert!(!retention.current(11002, origin + Duration::from_secs(8)));
        assert!(!retention.current(18000, origin + Duration::from_secs(7)));
        // The same retained object is checked before and after actual bytes.
        assert!(retention.current(11003, origin + Duration::from_millis(7900)));
        assert!(!retention.current(11004, origin + Duration::from_millis(8100)));
        let already_expired = OutputRetention::new(
            origin + Duration::from_secs(8),
            18000,
            18001,
            origin + Duration::from_secs(7),
        );
        assert!(!already_expired.current(17000, origin + Duration::from_secs(7)));
    }
}
