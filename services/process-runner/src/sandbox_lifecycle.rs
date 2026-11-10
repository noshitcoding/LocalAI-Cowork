//! Durable lifecycle for explicitly created workspaces. A repeated request ID reads
//! its receipt; an uncertain intent never issues its Kubernetes mutation again.
use crate::{
    ApiError, ApiResult, App, agent_sandbox, auth,
    config::{self, Configuration},
    failure, installed, installed_record, platform, workspace,
};
use anyhow::{Context, Result, ensure};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use cowork_agent_sandbox::runtime::Runtime;
use cowork_agent_sandbox::{ControlPlane, OwnedSandbox};
use cowork_contracts::ExecutionEnvironmentRef;
use cowork_domain::{canonical_hash, environments::EnvironmentDefinition};
use cowork_runner_protocol::{LaunchRequest, SandboxPreviewGrant, SandboxPreviewRequest};
use cowork_runner_protocol::{
    SandboxLifecycleOperation as Operation, SandboxLifecycleReceipt, SandboxLifecycleRequest,
    SandboxLifecycleState as StateKind, Signed,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub project_id: String,
    pub workspace_resource_id: String,
    pub binding_sha256: String,
    pub control_sha256: String,
    pub environment: ExecutionEnvironmentRef,
    pub definition: EnvironmentDefinition,
    pub sandbox: OwnedSandbox,
    pub pvc_name: String,
    pub pvc_uid: Option<String>,
    #[serde(default)]
    pub pvc_nonce: Option<String>,
    pub policy_uid: Option<String>,
    pub token_file: PathBuf,
    pub removed: bool,
    #[serde(default)]
    pub snapshot_sha256: Option<String>,
    #[serde(default)]
    pub last_active_at: i64,
}
fn path(config: &Configuration, workspace: &str) -> PathBuf {
    config
        .data_directory
        .join("sandbox-sessions")
        .join(format!("{}.json", canonical_hash(&workspace)))
}
fn save(config: &Configuration, session: &Session) -> Result<()> {
    config::durable_json(&path(config, &session.workspace_resource_id), session)
}
fn load(config: &Configuration, workspace: &str) -> Result<Session> {
    let mut s: Session = serde_json::from_slice(&config::private_read(
        &path(config, workspace),
        1024 * 1024,
    )?)?;
    ensure!(
        s.workspace_resource_id == workspace && s.binding_sha256 == canonical_hash(&config.binding),
        "Workspace session binding changed"
    );
    ensure!(
        s.control_sha256
            == agent_sandbox::control_hash(
                config
                    .agent_sandbox
                    .as_ref()
                    .context("Kubernetes configuration missing")?
            ),
        "Session Kubernetes configuration changed"
    );
    let activity = path(config, workspace).with_extension("activity");
    if activity.exists() {
        s.last_active_at =
            s.last_active_at
                .max(serde_json::from_slice::<i64>(&config::private_read(
                    &activity, 128,
                )?)?);
    }
    Ok(s)
}
fn touch(config: &Configuration, session: &Session) -> Result<()> {
    config::durable_json(
        &path(config, &session.workspace_resource_id).with_extension("activity"),
        &platform::now(),
    )
}
async fn suspend(
    config: &Configuration,
    plane: &ControlPlane,
    session: &mut Session,
) -> Result<()> {
    if session.sandbox.uid.is_none() {
        if let Some(sb) = plane.get("sandboxes", &session.sandbox.name).await? {
            plane.verify_owned(&sb, &session.sandbox)?;
            session.sandbox.uid =
                Some(cowork_agent_sandbox::control::field(&sb, "/metadata/uid")?.into());
            save(config, session)?;
        } else {
            return Ok(());
        }
    }
    let owned = session.sandbox.clone();
    plane
        .suspend(&owned, |receipt| {
            session.sandbox.stopped = Some(receipt.clone());
            save(config, session)
        })
        .await?;
    Ok(())
}
pub fn checkout(
    config: &Configuration,
    workspace: &str,
    definition: &EnvironmentDefinition,
) -> Result<(String, String)> {
    let mut session = load(config, workspace)
        .context("An explicit workspace sandbox Ensure is required before Kata execution")?;
    ensure!(
        !session.removed
            && session.definition == *definition
            && session.environment.definition_sha256 == canonical_hash(definition),
        "Session environment differs from frozen execution"
    );
    let plane = agent_sandbox::client(config)?;
    touch(config, &session)?;
    agent_sandbox::block_on(async {
        plane.inspect().await?;
        plane
            .verify_pvc(
                &session.pvc_name,
                session
                    .pvc_nonce
                    .as_deref()
                    .unwrap_or(&session.sandbox.nonce),
                session
                    .pvc_uid
                    .as_deref()
                    .context("Workspace PVC UID missing")?,
            )
            .await?;
        suspend(config, &plane, &mut session).await?;
        Ok(())
    })?;
    Ok((
        session.pvc_name,
        session.pvc_uid.context("Session PVC receipt absent")?,
    ))
}
fn original(config: &Configuration, id: &str) -> PathBuf {
    config
        .data_directory
        .join("sandbox-operations")
        .join(format!("{}.json", canonical_hash(&id)))
}
fn receipt_path(config: &Configuration, id: &str) -> PathBuf {
    config
        .data_directory
        .join("sandbox-operations")
        .join(format!("{}.receipt.json", canonical_hash(&id)))
}
fn existing_receipt(config: &Configuration, id: &str) -> Result<Signed<SandboxLifecycleReceipt>> {
    crate::key(id)?;
    let receipt: Signed<SandboxLifecycleReceipt> = serde_json::from_slice(&config::private_read(
        &receipt_path(config, id),
        1024 * 1024,
    )?)?;
    receipt.verify(
        "sandbox_lifecycle_receipt",
        &config.binding.runner_public_key,
    )?;
    ensure!(
        receipt.payload.request.request_id == id,
        "Lifecycle receipt identity mismatch"
    );
    Ok(receipt)
}
pub async fn receipt(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Signed<SandboxLifecycleReceipt>>> {
    auth(&app, &headers)?;
    if !receipt_path(&app.0.config, &id).exists() {
        return Err(ApiError(
            if original(&app.0.config, &id).exists() {
                StatusCode::CONFLICT
            } else {
                StatusCode::NOT_FOUND
            },
            "Lifecycle receipt not available",
        ));
    }
    existing_receipt(&app.0.config, &id)
        .map(Json)
        .map_err(failure)
}
fn preview_session(
    config: &Configuration,
    project: &str,
    workspace: &str,
    environment: &ExecutionEnvironmentRef,
) -> Result<Session> {
    let session = load(config, workspace)?;
    ensure!(
        !session.removed && session.project_id == project && session.environment == *environment,
        "Preview workspace scope changed"
    );
    Ok(session)
}
fn preview_candidates(config: &Configuration, session: &Session) -> Result<Vec<OwnedSandbox>> {
    let mut candidates = Vec::new();
    let mut count = 0;
    for entry in std::fs::read_dir(config.data_directory.join("attempts"))? {
        let entry = entry?;
        count += 1;
        ensure!(count <= 10000, "Attempt lookup exceeds bound");
        let request = entry.path().join("request.json");
        let owner = entry.path().join("ownership.json");
        if !request.exists() || !owner.exists() {
            continue;
        }
        let request: LaunchRequest =
            serde_json::from_slice(&config::private_read(&request, 1024 * 1024)?)?;
        if request.workspace_resource_id != session.workspace_resource_id
            || request.environment != session.environment
        {
            continue;
        }
        let owner: platform::Ownership =
            serde_json::from_slice(&config::private_read(&owner, 65536)?)?;
        if owner.adapter != "linux_kata" {
            continue;
        }
        ensure!(
            owner.owned_directory == entry.path()
                && owner.definition_sha256 == canonical_hash(&session.definition),
            "Preview attempt ownership changed"
        );
        if let Some(state) = owner.agent_sandbox
            && state.primary.stopped.is_none()
            && state.primary.pod.is_some()
        {
            candidates.push(state.primary);
        }
    }
    candidates.push(session.sandbox.clone());
    Ok(candidates)
}
pub async fn preview_issue(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<Signed<SandboxPreviewRequest>>,
) -> ApiResult<Json<Signed<SandboxPreviewGrant>>> {
    auth(&app, &headers)?;
    request
        .verify(
            "sandbox_preview_request",
            &app.0.config.binding.control_public_key,
        )
        .map_err(failure)?;
    request
        .payload
        .validate(&app.0.config.binding, platform::now())
        .map_err(failure)?;
    let worker = app.clone();
    let grant = tokio::task::spawn_blocking(move || -> Result<SandboxPreviewGrant> {
        let request = request.payload;
        let config = &worker.0.config;
        installed(&worker, &request.environment)?;
        let plane = agent_sandbox::client(config)?;
        ensure!(
            plane.config().preview_allowed_ports.contains(&request.port)
                && plane
                    .config()
                    .preview_gateway
                    .as_ref()
                    .is_some_and(|g| g.ports.contains(&request.port))
                && plane.config().preview_validator_token_file.is_some(),
            "Preview gateway/port is not configured"
        );
        let domain = plane
            .config()
            .preview_base_domain
            .as_deref()
            .context("Dedicated preview domain required")?;
        let session = preview_session(
            config,
            &request.project_id,
            &request.workspace_resource_id,
            &request.environment,
        )?;
        ensure!(
            session.sandbox.uid.as_deref() == Some(&request.expected_session_uid),
            "Persistent preview session UID changed"
        );
        let mut chosen = None;
        agent_sandbox::block_on(async {
            plane.inspect().await?;
            for owned in preview_candidates(config, &session)? {
                if owned.uid.is_none() || owned.pod.is_none() || owned.stopped.is_some() {
                    continue;
                }
                if let Ok(service) = plane.ready_target(&owned).await {
                    if owned.uid == session.sandbox.uid {
                        let workspace = workspace::Workspace::acquire(
                            &config.workspace_state_directory,
                            &session.workspace_resource_id,
                        )?;
                        ensure!(
                            session.snapshot_sha256.as_deref()
                                == Some(canonical_hash(&workspace.before).as_str()),
                            "Workspace changed since Ensure/Resume; refresh session before preview"
                        );
                    }
                    if tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        tokio::net::TcpStream::connect((service.as_str(), request.port)),
                    )
                    .await
                    .map_or(true, |r| r.is_err())
                    {
                        continue;
                    }
                    chosen = Some((owned, service));
                    touch(config, &session)?;
                    break;
                }
            }
            Ok(())
        })?;
        let (owned, service) = chosen.context("No current ready workspace Pod for preview")?;
        let preview_id = uuid::Uuid::new_v4().simple().to_string();
        let now = platform::now();
        Ok(SandboxPreviewGrant {
            preview_id: preview_id.clone(),
            runner_id: config.binding.runner_id.clone(),
            runner_generation: config.binding.generation,
            project_id: request.project_id,
            workspace_resource_id: request.workspace_resource_id,
            environment: request.environment,
            sandbox_uid: owned.uid.context("Sandbox UID absent")?,
            pod_uid: owned.pod.context("Pod UID absent")?.uid,
            namespace: plane.config().namespace.clone(),
            service_dns: service,
            port: request.port,
            preview_host: format!("{preview_id}.{domain}"),
            issued_at: now,
            expires_at: now + 60_000,
        })
    })
    .await
    .map_err(failure)?
    .map_err(failure)?;
    Ok(Json(
        app.0
            .signer
            .sign("sandbox_preview", grant)
            .map_err(failure)?,
    ))
}
pub async fn preview_validate(
    State(app): State<App>,
    headers: HeaderMap,
    Json(grant): Json<Signed<SandboxPreviewGrant>>,
) -> ApiResult<StatusCode> {
    // The gateway holds only this validator authority, never runner transport auth.
    let path = app
        .0
        .config
        .agent_sandbox
        .as_ref()
        .and_then(|c| c.preview_validator_token_file.as_ref())
        .ok_or(ApiError(
            StatusCode::UNAUTHORIZED,
            "Preview validator disabled",
        ))?;
    let expected = config::private_read(path, 16384).map_err(failure)?;
    if expected.len() < 48 {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "Preview validator credential invalid",
        ));
    }
    let presented = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or(ApiError(
            StatusCode::UNAUTHORIZED,
            "Preview validator authority required",
        ))?;
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"cowork-preview-validator");
    let tag = ring::hmac::sign(
        &key,
        String::from_utf8(expected)
            .map_err(failure)?
            .trim()
            .as_bytes(),
    );
    ring::hmac::verify(&key, presented.as_bytes(), tag.as_ref()).map_err(|_| {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "Preview validator authority denied",
        )
    })?;
    grant
        .verify("sandbox_preview", &app.0.config.binding.runner_public_key)
        .map_err(failure)?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let g = grant.payload;
        let now = platform::now();
        let plane = agent_sandbox::client(&app.0.config)?;
        let domain = plane
            .config()
            .preview_base_domain
            .as_deref()
            .context("Preview domain absent")?;
        ensure!(
            g.runner_id == app.0.config.binding.runner_id
                && g.runner_generation == app.0.config.binding.generation
                && g.issued_at <= now
                && g.expires_at > now
                && g.expires_at - g.issued_at <= 60_000
                && g.namespace == plane.config().namespace
                && g.preview_host == format!("{}.{}", g.preview_id, domain)
                && plane.config().preview_allowed_ports.contains(&g.port),
            "Preview grant expired or scope changed"
        );
        let session = preview_session(
            &app.0.config,
            &g.project_id,
            &g.workspace_resource_id,
            &g.environment,
        )?;
        agent_sandbox::block_on(async {
            plane.inspect().await?;
            for owned in preview_candidates(&app.0.config, &session)? {
                if owned.uid.as_deref() == Some(&g.sandbox_uid)
                    && owned.pod.as_ref().is_some_and(|p| p.uid == g.pod_uid)
                    && owned.stopped.is_none()
                {
                    ensure!(
                        plane.ready_target(&owned).await? == g.service_dns,
                        "Preview service changed"
                    );
                    if owned.uid == session.sandbox.uid {
                        let workspace = workspace::Workspace::acquire(
                            &app.0.config.workspace_state_directory,
                            &session.workspace_resource_id,
                        )?;
                        ensure!(
                            session.snapshot_sha256.as_deref()
                                == Some(canonical_hash(&workspace.before).as_str()),
                            "Workspace snapshot changed; old preview invalidated"
                        );
                    }
                    touch(&app.0.config, &session)?;
                    return Ok(());
                }
            }
            anyhow::bail!("Preview Pod is no longer the exact owned running Pod")
        })
    })
    .await
    .map_err(failure)?
    .map_err(failure)?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn lifecycle(
    state: State<App>,
    headers: HeaderMap,
    request: Json<Signed<SandboxLifecycleRequest>>,
) -> Response {
    match lifecycle_inner(state, headers, request).await {
        Ok(result) => result.into_response(),
        Err(error) => {
            let not_admitted = error.0 == StatusCode::TOO_EARLY;
            let absent = not_admitted && error.1 == "Sandbox session absent";
            let mut response = error.into_response();
            if not_admitted {
                response
                    .headers_mut()
                    .insert("x-cowork-admission", "not-admitted".parse().unwrap());
            }
            if absent {
                response
                    .headers_mut()
                    .insert("x-cowork-session", "absent".parse().unwrap());
            }
            response
        }
    }
}
async fn lifecycle_inner(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<Signed<SandboxLifecycleRequest>>,
) -> ApiResult<Json<Signed<SandboxLifecycleReceipt>>> {
    auth(&app, &headers)?;
    request
        .verify(
            "sandbox_lifecycle",
            &app.0.config.binding.control_public_key,
        )
        .map_err(failure)?;
    let intent = original(&app.0.config, &request.payload.request_id);
    if intent.exists() {
        let old: Signed<SandboxLifecycleRequest> =
            serde_json::from_slice(&config::private_read(&intent, 1024 * 1024).map_err(failure)?)
                .map_err(failure)?;
        if old != request {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "Lifecycle request ID is already bound",
            ));
        }
        return existing_receipt(&app.0.config, &request.payload.request_id)
            .map(Json)
            .map_err(failure);
    }
    request
        .payload
        .validate(&app.0.config.binding, platform::now())
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_EARLY,
                "Lifecycle authority expired before admission",
            )
        })?;
    if app.0.stopping.load(Ordering::Acquire) {
        return Err(ApiError(
            StatusCode::TOO_EARLY,
            "Receiver is stopping; operation not admitted",
        ));
    }
    let permit = app.0.serial.clone().try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::TOO_EARLY,
            "Receiver owns an active attempt; operation not admitted",
        )
    })?;
    // Detached owner survives HTTP cancellation. Only this owner may consume intent.
    let worker = app.clone();
    let result = tokio::spawn(async move {
        let _permit = permit;
        let config = worker.0.config.clone();
        let signed = request.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<SandboxLifecycleReceipt> {
            let fresh = matches!(
                signed.payload.operation,
                Operation::Ensure | Operation::Resume
            );
            let definition = installed_record(&worker, &signed.payload.environment, fresh)
                .map_err(|_| anyhow::Error::new(NotAdmitted))?
                .request
                .payload
                .definition;
            agent_sandbox::image(&definition).map_err(|_| anyhow::Error::new(NotAdmitted))?;
            preconditions(&config, &signed.payload, &definition)?;
            if !capacity(&config, &signed.payload).map_err(|_| anyhow::Error::new(NotAdmitted))? {
                return Err(anyhow::Error::new(NotAdmitted));
            }
            signed
                .payload
                .validate(&config.binding, platform::now())
                .map_err(|_| anyhow::Error::new(NotAdmitted))?;
            // create_new plus directory flush establishes exactly one operation owner.
            config::write_new(&intent, &serde_json::to_vec(&signed)?)?;
            config::sync_directory(intent.parent().context("Operation parent absent")?)?;
            apply(&config, &signed.payload, &definition)
        })
        .await;
        let receipt = result.map_err(|_| anyhow::anyhow!("Lifecycle supervisor failed"))??;
        let signed = app.0.signer.sign("sandbox_lifecycle_receipt", receipt)?;
        config::durable_json(
            &receipt_path(&app.0.config, &signed.payload.request.request_id),
            &signed,
        )?;
        Ok::<_, anyhow::Error>(signed)
    })
    .await
    .map_err(failure)?
    .map_err(|e| {
        if e.is::<SessionAbsent>() {
            ApiError(StatusCode::TOO_EARLY, "Sandbox session absent")
        } else if e.is::<NotAdmitted>() {
            ApiError(
                StatusCode::TOO_EARLY,
                "Lifecycle precondition rejected; operation not admitted",
            )
        } else {
            failure(e)
        }
    })?;
    Ok(Json(result))
}
#[derive(Debug)]
struct NotAdmitted;
impl std::fmt::Display for NotAdmitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Lifecycle precondition rejected before admission")
    }
}
impl std::error::Error for NotAdmitted {}
#[derive(Debug)]
struct SessionAbsent;
impl std::fmt::Display for SessionAbsent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sandbox session absent before admission")
    }
}
impl std::error::Error for SessionAbsent {}
fn preconditions(
    config: &Configuration,
    request: &SandboxLifecycleRequest,
    definition: &EnvironmentDefinition,
) -> Result<()> {
    if !path(config, &request.workspace_resource_id).exists() {
        if request.operation == Operation::Inspect {
            return Err(anyhow::Error::new(SessionAbsent));
        }
        ensure!(
            request.operation == Operation::Ensure && request.expected_sandbox_uid.is_none(),
            NotAdmitted
        );
        return Ok(());
    }
    let session = load(config, &request.workspace_resource_id)
        .map_err(|_| anyhow::Error::new(NotAdmitted))?;
    validate_session_authority(&session, request, definition, true)
        .map_err(|_| anyhow::Error::new(NotAdmitted))?;
    if session.removed && matches!(request.operation, Operation::Ensure | Operation::Inspect) {
        let observed = (|| -> Result<()> {
            let plane = agent_sandbox::client(config)?;
            agent_sandbox::block_on(plane.inspect())?;
            ensure!(
                state(&plane, &session)? == StateKind::Stopped,
                "Previous Sandbox removal not yet observed"
            );
            agent_sandbox::block_on(
                plane.verify_pvc(
                    &session.pvc_name,
                    session
                        .pvc_nonce
                        .as_deref()
                        .unwrap_or(&session.sandbox.nonce),
                    session
                        .pvc_uid
                        .as_deref()
                        .context("Retained PVC UID missing")?,
                ),
            )?;
            Ok(())
        })();
        observed.map_err(|_| anyhow::Error::new(NotAdmitted))?;
    }
    Ok(())
}
fn validate_session_authority(
    session: &Session,
    request: &SandboxLifecycleRequest,
    definition: &EnvironmentDefinition,
    existing: bool,
) -> Result<()> {
    let restarting = session.removed && request.operation == Operation::Ensure;
    let stopped_inspect = session.removed && request.operation == Operation::Inspect;
    ensure!(
        session.project_id == request.project_id
            && session.workspace_resource_id == request.workspace_resource_id
            && (restarting
                || stopped_inspect
                || (session.environment == request.environment
                    && session.definition == *definition)),
        "Lifecycle workspace or environment changed"
    );
    if let Some(expected) = &request.expected_sandbox_uid {
        ensure!(
            session.sandbox.uid.as_ref() == Some(expected),
            "Lifecycle expected Sandbox UID changed"
        );
    }
    if existing
        && request.operation != Operation::Inspect
        && !(restarting && request.expected_sandbox_uid.is_none())
    {
        ensure!(
            request.expected_sandbox_uid.is_some()
                && request.expected_sandbox_uid == session.sandbox.uid,
            "Mutating an existing session requires exact Sandbox UID"
        );
    }
    ensure!(
        !(session.removed && request.operation == Operation::Resume),
        "Stopped session requires explicit Ensure"
    );
    if restarting || stopped_inspect {
        ensure!(
            session.sandbox.uid.is_some()
                && session.sandbox.pod.as_ref().is_none_or(|pod| session
                    .sandbox
                    .stopped
                    .as_ref()
                    .is_some_and(|receipt| receipt.sandbox_uid
                        == session.sandbox.uid.as_deref().unwrap_or("")
                        && &receipt.pod == pod
                        && !receipt.terminated_container_ids.is_empty()))
                && session.pvc_uid.is_some(),
            "Restart requires previous physical stop and retained PVC identity"
        );
    }
    Ok(())
}
fn capacity(config: &Configuration, request: &SandboxLifecycleRequest) -> Result<bool> {
    if !matches!(request.operation, Operation::Ensure | Operation::Resume) {
        return Ok(true);
    }
    let plane = agent_sandbox::client(config)?;
    let mut count = 0usize;
    for entry in std::fs::read_dir(config.data_directory.join("sandbox-sessions"))? {
        let entry = entry?;
        if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let stored: Session =
            serde_json::from_slice(&config::private_read(&entry.path(), 1024 * 1024)?)?;
        if stored.removed || stored.workspace_resource_id == request.workspace_resource_id {
            continue;
        }
        let stored = load(config, &stored.workspace_resource_id)?;
        if !matches!(
            state(&plane, &stored)?,
            StateKind::Suspended | StateKind::Stopped
        ) {
            count += 1;
        }
    }
    Ok(count < plane.config().max_active_sessions)
}
fn state(plane: &ControlPlane, session: &Session) -> Result<StateKind> {
    agent_sandbox::block_on(async {
        let Some(sb) = plane.get("sandboxes", &session.sandbox.name).await? else {
            return Ok(if session.removed {
                StateKind::Stopped
            } else {
                StateKind::Unknown
            });
        };
        plane.verify_owned(&sb, &session.sandbox)?;
        if sb.pointer("/metadata/deletionTimestamp").is_some() {
            return Ok(StateKind::Stopping);
        }
        let conditions = sb
            .pointer("/status/conditions")
            .and_then(serde_json::Value::as_array);
        let has = |name: &str| {
            conditions.is_some_and(|a| {
                a.iter().any(|v| {
                    v["type"] == name
                        && v["status"] == "True"
                        && v["observedGeneration"].as_u64().is_some()
                        && v["observedGeneration"] == sb["metadata"]["generation"]
                })
            })
        };
        Ok(if has("Ready") {
            StateKind::Ready
        } else if has("Suspended")
            && session
                .sandbox
                .pod
                .as_ref()
                .is_none_or(|_| session.sandbox.stopped.is_some())
        {
            StateKind::Suspended
        } else {
            StateKind::Provisioning
        })
    })
}
fn apply(
    config: &Configuration,
    request: &SandboxLifecycleRequest,
    definition: &EnvironmentDefinition,
) -> Result<SandboxLifecycleReceipt> {
    let plane = agent_sandbox::client(config)?;
    if matches!(request.operation, Operation::Ensure | Operation::Resume) {
        plane
            .config()
            .approve_image(agent_sandbox::image(definition)?)?;
    }
    agent_sandbox::block_on(plane.inspect())?;
    let existing = path(config, &request.workspace_resource_id).exists();
    let mut session = if existing {
        load(config, &request.workspace_resource_id)?
    } else {
        ensure!(
            request.operation == Operation::Ensure && request.expected_sandbox_uid.is_none(),
            "Create workspace session before lifecycle operation"
        );
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let token_file = config
            .data_directory
            .join("sandbox-sessions")
            .join(format!("{nonce}.token"));
        config::write_new(&token_file, agent_sandbox::token().as_bytes())?;
        let s = Session {
            project_id: request.project_id.clone(),
            workspace_resource_id: request.workspace_resource_id.clone(),
            binding_sha256: canonical_hash(&config.binding),
            control_sha256: agent_sandbox::control_hash(plane.config()),
            environment: request.environment.clone(),
            definition: definition.clone(),
            sandbox: OwnedSandbox {
                name: format!("cws-{nonce}"),
                nonce: nonce.clone(),
                uid: None,
                pod: None,
                stopped: None,
            },
            pvc_name: format!("cw-data-{nonce}"),
            pvc_uid: None,
            pvc_nonce: Some(nonce.clone()),
            policy_uid: None,
            token_file,
            removed: false,
            snapshot_sha256: None,
            last_active_at: platform::now(),
        };
        save(config, &s)?;
        s
    };
    let restarting = session.removed && request.operation == Operation::Ensure;
    validate_session_authority(&session, request, definition, existing)?;
    if restarting {
        ensure!(
            session.sandbox.pod.is_none() || session.sandbox.stopped.is_some(),
            "Restart requires previous positive physical stop receipt"
        );
        agent_sandbox::block_on(
            plane.verify_pvc(
                &session.pvc_name,
                session
                    .pvc_nonce
                    .as_deref()
                    .unwrap_or(&session.sandbox.nonce),
                session
                    .pvc_uid
                    .as_deref()
                    .context("Retained workspace PVC UID missing")?,
            ),
        )?;
        config::durable_json(
            &config
                .data_directory
                .join("sandbox-operations")
                .join(format!(
                    "{}.previous-session.json",
                    canonical_hash(&request.request_id)
                )),
            &session,
        )?;
        agent_sandbox::block_on(plane.delete_app_resource(
            "networkpolicies",
            &session.sandbox.name,
            &session.sandbox.nonce,
            session.policy_uid.as_deref(),
        ))?;
        rotate_stopped_identity(
            &config.data_directory.join("sandbox-sessions"),
            &mut session,
            &request.environment,
            definition,
        )?;
        save(config, &session)?;
    }
    match request.operation {
        Operation::Ensure | Operation::Resume => {
            ensure!(
                !session.removed,
                "Stopped session requires a new explicit workspace identity"
            );
            agent_sandbox::block_on(async {
                if !existing || restarting {
                    if !existing {
                        session.pvc_uid = Some(
                            plane
                                .create_pvc(
                                    &session.pvc_name,
                                    &session.sandbox.nonce,
                                    definition.limits.scratch_bytes,
                                )
                                .await?,
                        );
                    }
                    save(config, &session)?;
                    session.policy_uid = Some(
                        plane
                            .create_policy(
                                &session.sandbox.name,
                                &session.sandbox.nonce,
                                agent_sandbox::proxy(&plane, definition)?,
                            )
                            .await?,
                    );
                    save(config, &session)?;
                    let token =
                        String::from_utf8(config::private_read(&session.token_file, 16384)?)?;
                    let uid = plane
                        .create(
                            &session.sandbox,
                            &session.pvc_name,
                            agent_sandbox::image(definition)?,
                            &agent_sandbox::resources(definition),
                            &workspace::digest(token.as_bytes()),
                            false,
                        )
                        .await?;
                    session.sandbox.uid = Some(uid);
                    save(config, &session)?;
                } else {
                    let observed = state(&plane, &session)?;
                    if observed == StateKind::Ready {
                        suspend(config, &plane, &mut session).await?;
                    }
                    ensure!(
                        matches!(observed, StateKind::Suspended | StateKind::Ready),
                        "Session state remains unconfirmed"
                    );
                    session.sandbox.pod = None;
                    session.sandbox.stopped = None;
                    save(config, &session)?;
                    plane.resume(&session.sandbox).await?;
                }
                let (pod, host) = plane
                    .wait_ready(&session.sandbox, agent_sandbox::image(definition)?)
                    .await?;
                plane
                    .verify_policy(
                        &session.sandbox.name,
                        &session.sandbox.nonce,
                        session
                            .policy_uid
                            .as_deref()
                            .context("Policy UID receipt missing")?,
                        agent_sandbox::proxy(&plane, definition)?,
                    )
                    .await?;
                session.sandbox.pod = Some(pod);
                save(config, &session)?;
                plane.fence_pod(&session.sandbox).await?;
                let token = String::from_utf8(config::private_read(&session.token_file, 16384)?)?;
                let mut runtime = Runtime::connect(&host, &token).await?;
                let proof = config
                    .data_directory
                    .join("sandbox-operations")
                    .join(format!("proof-{}", canonical_hash(&request.request_id)));
                config::private_directory(&proof)?;
                agent_sandbox::boundary(config, definition, &mut runtime, &plane, &proof).await?;
                let workspace = workspace::Workspace::acquire(
                    &config.workspace_state_directory,
                    &session.workspace_resource_id,
                )?;
                agent_sandbox::stage(
                    &mut runtime,
                    &workspace.root,
                    &workspace.before,
                    &std::sync::atomic::AtomicBool::new(false),
                )
                .await?;
                session.snapshot_sha256 = Some(canonical_hash(&workspace.before));
                save(config, &session)?;
                touch(config, &session)?;
                if plane.config().preview_base_domain.is_some()
                    && plane.config().preview_allowed_ports.contains(&8081)
                {
                    let start = runtime
                        .execute(
                            vec![
                                "/usr/local/bin/cowork-runner-probe".into(),
                                "--start-preview".into(),
                                "8081".into(),
                            ],
                            std::collections::HashMap::new(),
                            std::time::Duration::from_secs(10),
                            &std::sync::atomic::AtomicBool::new(false),
                            65536,
                        )
                        .await?;
                    ensure!(
                        start.complete && start.exit_code == Some(0),
                        "Trusted workspace preview server failed to start"
                    );
                }
                Ok(())
            })?;
        }
        Operation::Suspend => agent_sandbox::block_on(suspend(config, &plane, &mut session))?,
        Operation::Stop => agent_sandbox::block_on(async {
            suspend(config, &plane, &mut session).await?;
            plane.delete_sandbox(&session.sandbox).await?;
            session.removed = true;
            save(config, &session)
        })?,
        Operation::Inspect => {}
    }
    let state = state(&plane, &session)?;
    Ok(SandboxLifecycleReceipt {
        request: request.clone(),
        sandbox_name: session.sandbox.name.clone(),
        sandbox_uid: session.sandbox.uid.clone(),
        pod_uid: if matches!(state, StateKind::Ready) {
            session.sandbox.pod.as_ref().map(|p| p.uid.clone())
        } else {
            None
        },
        workspace_pvc: session.pvc_name,
        state,
        observed_at: platform::now(),
    })
}
fn rotate_stopped_identity(
    directory: &std::path::Path,
    session: &mut Session,
    environment: &ExecutionEnvironmentRef,
    definition: &EnvironmentDefinition,
) -> Result<()> {
    ensure!(
        session.removed,
        "Only a known stopped session may rotate identity"
    );
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    session.pvc_nonce = Some(
        session
            .pvc_nonce
            .clone()
            .unwrap_or(session.sandbox.nonce.clone()),
    );
    session.token_file = directory.join(format!("{nonce}.token"));
    config::write_new(&session.token_file, agent_sandbox::token().as_bytes())?;
    session.sandbox = OwnedSandbox {
        name: format!("cws-{nonce}"),
        nonce,
        uid: None,
        pod: None,
        stopped: None,
    };
    session.removed = false;
    session.snapshot_sha256 = None;
    session.policy_uid = None;
    session.environment = environment.clone();
    session.definition = definition.clone();
    Ok(())
}
pub fn recover(config: &Configuration) -> Result<()> {
    if config.agent_sandbox.is_none() {
        return Ok(());
    }
    let plane = agent_sandbox::client(config)?;
    agent_sandbox::block_on(async {
        plane.inspect().await?;
        for entry in std::fs::read_dir(config.data_directory.join("sandbox-sessions"))? {
            let entry = entry?;
            if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let mut s: Session =
                serde_json::from_slice(&config::private_read(&entry.path(), 1024 * 1024)?)?;
            ensure!(
                s.binding_sha256 == canonical_hash(&config.binding)
                    && s.control_sha256 == agent_sandbox::control_hash(plane.config())
                    && path(config, &s.workspace_resource_id) == entry.path(),
                "Foreign workspace journal"
            );
            suspend(config, &plane, &mut s).await?;
        }
        Ok(())
    })
}
/// Called only while the receiver owns its existing global serial permit.
pub async fn maintain_idle(app: &App) -> Result<()> {
    let config = app.0.config.clone();
    if config.agent_sandbox.is_none() {
        return Ok(());
    }
    tokio::task::spawn_blocking(move || -> Result<()> {
        let plane = agent_sandbox::client(&config)?;
        let now = platform::now();
        for entry in std::fs::read_dir(config.data_directory.join("sandbox-sessions"))? {
            let entry = entry?;
            if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let stored: Session =
                serde_json::from_slice(&config::private_read(&entry.path(), 1024 * 1024)?)?;
            let mut session = load(&config, &stored.workspace_resource_id)?;
            if session.removed
                || now.saturating_sub(session.last_active_at)
                    < (plane.config().idle_suspend_seconds as i64) * 1000
            {
                continue;
            }
            if state(&plane, &session)? != StateKind::Ready {
                continue;
            }
            let Ok(_workspace) = workspace::Workspace::acquire(
                &config.workspace_state_directory,
                &session.workspace_resource_id,
            ) else {
                continue;
            };
            let current = load(&config, &session.workspace_resource_id)?;
            if platform::now().saturating_sub(current.last_active_at)
                < (plane.config().idle_suspend_seconds as i64) * 1000
            {
                continue;
            }
            agent_sandbox::block_on(suspend(&config, &plane, &mut session))?;
        }
        Ok(())
    })
    .await
    .context("Idle supervisor failed")?
}

#[cfg(test)]
mod lifecycle_admission_tests {
    use super::*;
    use cowork_agent_sandbox::{PodIdentity, StopReceipt};
    fn fixture(directory: &std::path::Path) -> (Session, SandboxLifecycleRequest) {
        let definition = serde_json::from_value(serde_json::json!({"format_version":2,"artifact":{"kind":"linux_kata_image","reference":format!("registry.example/runtime@sha256:{}","b".repeat(64))},"programs":{"python":"usr/bin/python3"},"packages":[],"limits":{"cpu_millis":500,"memory_bytes":268435456,"processes":64,"scratch_bytes":268435456},"network":{"kind":"denied"}})).unwrap();
        let environment = ExecutionEnvironmentRef {
            environment_id: "environment".into(),
            version_id: "v1".into(),
            definition_sha256: canonical_hash(&definition),
            qualification_sha256: "c".repeat(64),
        };
        let pod = PodIdentity {
            name: "sandbox".into(),
            uid: "original-pod".into(),
            node: "node".into(),
        };
        let session = Session {
            project_id: "project".into(),
            workspace_resource_id: "workspace".into(),
            binding_sha256: "a".repeat(64),
            control_sha256: "b".repeat(64),
            environment: environment.clone(),
            definition,
            sandbox: OwnedSandbox {
                name: "sandbox".into(),
                nonce: "d".repeat(32),
                uid: Some("original-sandbox".into()),
                pod: Some(pod.clone()),
                stopped: Some(StopReceipt {
                    sandbox_uid: "original-sandbox".into(),
                    pod,
                    terminated_container_ids: vec!["containerd://original".into()],
                }),
            },
            pvc_name: "retained-pvc".into(),
            pvc_uid: Some("original-pvc".into()),
            pvc_nonce: Some("e".repeat(32)),
            policy_uid: Some("old-policy".into()),
            token_file: directory.join("old-token"),
            removed: false,
            snapshot_sha256: None,
            last_active_at: 1,
        };
        let request = SandboxLifecycleRequest {
            request_id: "operation".into(),
            runner_id: "runner".into(),
            runner_generation: 1,
            adapter_build_sha256: "a".repeat(64),
            project_id: "project".into(),
            workspace_resource_id: "workspace".into(),
            environment,
            operation: Operation::Ensure,
            expected_sandbox_uid: None,
            issued_at: 1,
            expires_at: 2,
        };
        (session, request)
    }
    #[test]
    fn known_session_preconditions_reject_missing_and_wrong_uid_before_intent() {
        let directory = tempfile::tempdir().unwrap();
        let (mut session, mut request) = fixture(directory.path());
        assert!(validate_session_authority(&session, &request, &session.definition, true).is_err());
        request.operation = Operation::Suspend;
        request.expected_sandbox_uid = Some("wrong-uid".into());
        assert!(validate_session_authority(&session, &request, &session.definition, true).is_err());
        request.expected_sandbox_uid = Some("original-sandbox".into());
        assert!(validate_session_authority(&session, &request, &session.definition, true).is_ok());
        session.removed = true;
        request.operation = Operation::Resume;
        assert!(validate_session_authority(&session, &request, &session.definition, true).is_err());
        request.operation = Operation::Ensure;
        request.expected_sandbox_uid = None;
        assert!(validate_session_authority(&session, &request, &session.definition, true).is_ok());
        request.environment.environment_id = "new-environment".into();
        request.operation = Operation::Inspect;
        assert!(
            validate_session_authority(&session, &request, &session.definition, true).is_ok(),
            "Stopped Inspect may observe retained custody under selected new environment"
        );
        session.removed = false;
        assert!(
            validate_session_authority(&session, &request, &session.definition, true).is_err(),
            "Live or suspended session cannot change environment through Inspect"
        );
        session.removed = true;
        request.project_id = "different-project".into();
        assert!(validate_session_authority(&session, &request, &session.definition, true).is_err());
        assert_eq!(
            std::fs::read_dir(directory.path()).unwrap().count(),
            0,
            "Preconditions must be read-only"
        );
    }
    #[test]
    fn stopped_restart_rotates_identity_and_token_retaining_exact_pvc_custody() {
        let directory = tempfile::tempdir().unwrap();
        let (mut session, request) = fixture(directory.path());
        session.removed = true;
        config::write_new(&session.token_file, b"old-secret").unwrap();
        let previous = session.clone();
        let definition = session.definition.clone();
        rotate_stopped_identity(
            directory.path(),
            &mut session,
            &request.environment,
            &definition,
        )
        .unwrap();
        assert_ne!(session.sandbox.name, previous.sandbox.name);
        assert_ne!(session.sandbox.nonce, previous.sandbox.nonce);
        assert!(
            session.sandbox.uid.is_none()
                && session.sandbox.pod.is_none()
                && session.sandbox.stopped.is_none()
        );
        assert_eq!(session.pvc_name, previous.pvc_name);
        assert_eq!(session.pvc_uid, previous.pvc_uid);
        assert_eq!(session.pvc_nonce, previous.pvc_nonce);
        assert_ne!(session.token_file, previous.token_file);
        assert_ne!(
            config::private_read(&session.token_file, 1024).unwrap(),
            b"old-secret"
        );
        assert_eq!(
            config::private_read(&previous.token_file, 1024).unwrap(),
            b"old-secret"
        );
    }
}
