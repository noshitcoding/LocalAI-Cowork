//! Controller side of the private managed-input transport. Every byte must
//! remain covered by the original confirmed input source and launch envelope.
use super::*;
use base64::Engine;
use cowork_contracts::{ReadWorkspaceTransferChunk, WorkspaceTransferManifest};
use cowork_domain::{hash, workspace_transfer};
use cowork_runner_protocol::{
    ExecutionState, MANAGED_BEGIN_BODY_BYTES, MANAGED_CHUNK_BODY_BYTES, MANAGED_INPUTS_PATH,
    MANAGED_READY_PATH, MANAGED_RESPONSE_BYTES, ManagedBeginRequest, ManagedBeginResponse,
    ManagedChunkAck, ManagedChunkRequest, ManagedInputOperation, ManagedMissingResponse,
    ManagedReadiness, ManagedReadyRequest, envelope_hash,
};
use cowork_runtime::ManagedInputManifest;
use std::{collections::BTreeSet, time::Instant};

// Keep transport diagnostics bounded to a static phase and the public error code.
// Source bytes, tickets, grants and private HTTP data must never be logged.
fn handoff_failure(phase: &'static str) -> impl FnOnce(Error) -> Error {
    move |error| {
        tracing::warn!(phase, code = error.code(), "Managed input handoff failed");
        error
    }
}

impl HttpsRunner {
    async fn managed_post<T: Serialize>(
        &self,
        path: &str,
        value: &T,
        maximum: usize,
    ) -> Result<reqwest::Response> {
        let bytes = serde_json::to_vec(value).map_err(|_| Error::Forbidden)?;
        if bytes.len() > maximum {
            return Err(Error::Forbidden);
        }
        let url = self.origin.join(path).map_err(|_| Error::Forbidden)?;
        self.client
            .post(url)
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(bytes)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| Error::UncertainEffect)
    }

    async fn managed_body<T: DeserializeOwned>(
        mut response: reqwest::Response,
        allowed: &[reqwest::StatusCode],
    ) -> Result<T> {
        if !allowed.contains(&response.status()) {
            // No body, query, headers or authority material enters diagnostics.
            // The configured origin forbids URL credentials and query strings.
            tracing::warn!(
                status = response.status().as_u16(),
                path = response.url().path(),
                "Managed runner response was not accepted"
            );
            return Err(Error::UncertainEffect);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MANAGED_RESPONSE_BYTES as u64)
        {
            return Err(Error::UncertainEffect);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::UncertainEffect)? {
            if bytes.len().saturating_add(chunk.len()) > MANAGED_RESPONSE_BYTES {
                return Err(Error::UncertainEffect);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| Error::UncertainEffect)
    }

    pub(super) async fn managed_readiness(
        &self,
        environment: &ExecutionEnvironmentRef,
        target: &cowork_contracts::ExecutionTargetRef,
    ) -> Result<Signed<ManagedReadiness>> {
        let request = ManagedReadyRequest {
            format_version: 1,
            request_id: uuid::Uuid::new_v4().to_string(),
            environment: environment.clone(),
            target: target.clone(),
        };
        self.managed_readiness_request(request).await
    }

    pub(super) async fn managed_readiness_request(
        &self,
        request: ManagedReadyRequest,
    ) -> Result<Signed<ManagedReadiness>> {
        let started = Instant::now();
        let response = self
            .managed_post(MANAGED_READY_PATH, &request, 65536)
            .await?;
        let signed: Signed<ManagedReadiness> =
            Self::managed_body(response, &[reqwest::StatusCode::OK]).await?;
        signed
            .verify("managed_readiness", &self.binding.runner_public_key)
            .map_err(|_| Error::Forbidden)?;
        signed
            .payload
            .validate(&request, &self.binding, now()?)
            .map_err(|_| Error::Forbidden)?;
        if started.elapsed()
            >= Duration::from_millis(cowork_runner_protocol::MANAGED_READINESS_MILLIS as u64)
        {
            return Err(Error::Forbidden);
        }
        Ok(signed)
    }

    pub(super) async fn managed_launch_once(
        &self,
        request: Signed<LaunchRequest>,
        source: Arc<dyn cowork_runtime::ManagedInputAccess>,
    ) -> Result<Signed<Receipt>> {
        let manifest = source
            .manifest()
            .await
            .map_err(handoff_failure("source_manifest"))?;
        let input = Input::new(&self.binding, &request, manifest)
            .map_err(handoff_failure("source_binding"))?;
        let readiness = self
            .managed_readiness(
                &request.payload.environment,
                &input.source.grant.payload.target,
            )
            .await
            .map_err(handoff_failure("readiness"))?;
        input
            .fresh(&self.binding, &request)
            .map_err(handoff_failure("before_begin"))?;
        let begin = ManagedBeginRequest {
            format_version: 1,
            readiness,
            launch: request.clone(),
            grant: input.source.grant.clone(),
            manifest: input.source.manifest.clone(),
        };
        let response = self
            .managed_post(MANAGED_INPUTS_PATH, &begin, MANAGED_BEGIN_BODY_BYTES)
            .await
            .map_err(handoff_failure("begin_send"))?;
        let response: ManagedBeginResponse = Self::managed_body(
            response,
            &[
                reqwest::StatusCode::OK,
                reqwest::StatusCode::CREATED,
                reqwest::StatusCode::ACCEPTED,
            ],
        )
        .await
        .map_err(handoff_failure("begin_response"))?;
        let ticket = match response {
            ManagedBeginResponse::Execution { receipt } => {
                let receipt = verify_receipt(&self.binding, &request, receipt)
                    .map_err(handoff_failure("replayed_receipt"))?;
                return self
                    .managed_poll(&request, receipt, input.deadline)
                    .await
                    .map_err(handoff_failure("replayed_poll"));
            }
            ManagedBeginResponse::Input { ticket } => ticket,
        };
        ticket
            .verify("managed_input_ticket", &self.binding.runner_public_key)
            .map_err(|_| Error::UncertainEffect)
            .map_err(handoff_failure("ticket_signature"))?;
        ticket
            .payload
            .validate(&begin, now().map_err(handoff_failure("ticket_clock"))?)
            .map_err(|_| Error::UncertainEffect)
            .map_err(handoff_failure("ticket_binding"))?;
        let operation = ManagedInputOperation {
            ticket: ticket.clone(),
        };
        let prefix = format!("{MANAGED_INPUTS_PATH}/{}", ticket.payload.input_id);
        input
            .fresh(&self.binding, &request)
            .map_err(handoff_failure("before_missing"))?;
        let response = self
            .managed_post(&format!("{prefix}/missing"), &operation, 65536)
            .await
            .map_err(handoff_failure("missing_send"))?;
        let missing: ManagedMissingResponse =
            Self::managed_body(response, &[reqwest::StatusCode::OK])
                .await
                .map_err(handoff_failure("missing_response"))?;
        missing
            .validate(&ticket, &input.source.manifest)
            .map_err(|_| Error::UncertainEffect)
            .map_err(handoff_failure("missing_binding"))?;
        validate_missing(&input.source.manifest, &missing.missing)
            .map_err(handoff_failure("missing_chunks"))?;
        for chunk in missing.missing {
            input
                .fresh(&self.binding, &request)
                .map_err(handoff_failure("before_source_chunk"))?;
            let bytes = source
                .chunk(chunk.clone())
                .await
                .map_err(handoff_failure("source_chunk"))?
                .bytes;
            validate_chunk(&input.source.manifest, &chunk, &bytes)
                .map_err(handoff_failure("chunk_binding"))?;
            input
                .fresh(&self.binding, &request)
                .map_err(handoff_failure("before_chunk_send"))?;
            let value = ManagedChunkRequest {
                ticket: ticket.clone(),
                chunk: chunk.clone(),
                bytes_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            };
            let response = self
                .managed_post(
                    &format!("{prefix}/chunks"),
                    &value,
                    MANAGED_CHUNK_BODY_BYTES,
                )
                .await
                .map_err(handoff_failure("chunk_send"))?;
            let ack: ManagedChunkAck = Self::managed_body(response, &[reqwest::StatusCode::OK])
                .await
                .map_err(handoff_failure("chunk_response"))?;
            if ack.path != chunk.path
                || ack.offset != chunk.offset
                || ack.size != bytes.len() as u64
                || ack.sha256 != chunk.expected_sha256
                || !workspace_transfer::valid_sha256(&ack.receipt_sha256)
            {
                return Err(handoff_failure("chunk_ack")(Error::UncertainEffect));
            }
        }
        // Refresh source custody after the final send, including an empty input.
        // A received ticket alone never substitutes for current controller rights.
        let fresh = source
            .manifest()
            .await
            .map_err(handoff_failure("final_source_manifest"))?;
        if fresh.grant != input.source.grant || fresh.manifest != input.source.manifest {
            return Err(handoff_failure("final_source_binding")(
                Error::ExecutionPlanChanged,
            ));
        }
        input
            .fresh(&self.binding, &request)
            .map_err(handoff_failure("before_start"))?;
        ticket
            .payload
            .validate(
                &begin,
                now().map_err(handoff_failure("final_ticket_clock"))?,
            )
            .map_err(|_| Error::UncertainEffect)
            .map_err(handoff_failure("final_ticket_binding"))?;
        // Exactly one start POST. Even a lost reply must be reconciled through
        // the existing receipt endpoint, never by resending this start.
        let response = self
            .managed_post(&format!("{prefix}/start"), &operation, 65536)
            .await
            .map_err(handoff_failure("start_send"))?;
        let receipt: Signed<Receipt> = Self::managed_body(
            response,
            &[reqwest::StatusCode::OK, reqwest::StatusCode::ACCEPTED],
        )
        .await
        .map_err(handoff_failure("start_response"))?;
        let receipt = verify_receipt(&self.binding, &request, receipt)
            .map_err(handoff_failure("start_receipt"))?;
        self.managed_poll(&request, receipt, input.deadline)
            .await
            .map_err(handoff_failure("completion_poll"))
    }

    async fn managed_poll(
        &self,
        request: &Signed<LaunchRequest>,
        mut receipt: Signed<Receipt>,
        deadline: Instant,
    ) -> Result<Signed<Receipt>> {
        let id = &request.payload.process_attempt_id;
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        {
            return Err(Error::UncertainEffect);
        }
        let url = self
            .origin
            .join(&format!("v2/receipts/{id}"))
            .map_err(|_| Error::UncertainEffect)?;
        let mut last_revision = receipt.payload.revision;
        loop {
            if matches!(receipt.payload.state, ExecutionState::Completed { .. }) {
                return Ok(receipt);
            }
            if Instant::now() >= deadline || now()? >= request.payload.expires_at {
                return Err(Error::UncertainEffect);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            let response = self
                .client
                .get(url.clone())
                .bearer_auth(&self.token)
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .map_err(|_| Error::UncertainEffect)?;
            receipt = verify_receipt(
                &self.binding,
                request,
                Self::managed_body(
                    response,
                    &[reqwest::StatusCode::OK, reqwest::StatusCode::ACCEPTED],
                )
                .await?,
            )?;
            if receipt.payload.revision < last_revision {
                return Err(Error::UncertainEffect);
            }
            last_revision = receipt.payload.revision;
        }
    }
}

mod results;

struct Input {
    source: ManagedInputManifest,
    deadline: Instant,
}

fn now() -> Result<i64> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error::Forbidden)?;
    i64::try_from(elapsed.as_millis()).map_err(|_| Error::Forbidden)
}

impl Input {
    fn new(
        binding: &Binding,
        request: &Signed<LaunchRequest>,
        source: ManagedInputManifest,
    ) -> Result<Self> {
        let at = now()?;
        validate_source(binding, request, &source, at)?;
        let remaining = source
            .grant
            .payload
            .initial_execution_deadline(&request.payload)
            .checked_sub(at)
            .filter(|v| *v > 0)
            .ok_or(Error::Forbidden)?;
        let input = Self {
            source,
            deadline: Instant::now()
                .checked_add(Duration::from_millis(remaining as u64))
                .ok_or(Error::Forbidden)?,
        };
        input.fresh(binding, request)?;
        Ok(input)
    }

    fn fresh(&self, binding: &Binding, request: &Signed<LaunchRequest>) -> Result<()> {
        if Instant::now() >= self.deadline {
            return Err(Error::Forbidden);
        }
        self.source
            .grant
            .payload
            .validate_for_launch(
                binding,
                &self.source.grant.payload.target,
                &request.payload,
                now()?,
            )
            .map_err(|_| Error::Forbidden)
    }
}

fn validate_source(
    binding: &Binding,
    request: &Signed<LaunchRequest>,
    source: &ManagedInputManifest,
    at: i64,
) -> Result<()> {
    request
        .verify("launch", &binding.control_public_key)
        .map_err(|_| Error::Forbidden)?;
    source
        .grant
        .verify("managed_input_grant", &binding.control_public_key)
        .map_err(|_| Error::Forbidden)?;
    source
        .grant
        .payload
        .validate_for_launch(binding, &source.grant.payload.target, &request.payload, at)
        .map_err(|_| Error::Forbidden)?;
    workspace_transfer::validate(&source.manifest)?;
    if source.manifest.sha256 != source.grant.payload.input_manifest_sha256
        || source.grant.payload.expires_at > request.payload.expires_at
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

fn declared_chunk<'a>(
    manifest: &'a WorkspaceTransferManifest,
    request: &ReadWorkspaceTransferChunk,
) -> Result<&'a cowork_contracts::WorkspaceTransferChunk> {
    workspace_transfer::validate_path(&request.path)?;
    manifest
        .files
        .iter()
        .find(|file| file.path == request.path)
        .and_then(|file| {
            file.chunks.iter().find(|chunk| {
                chunk.offset == request.offset && chunk.sha256 == request.expected_sha256
            })
        })
        .ok_or(Error::Forbidden)
}

fn validate_missing(
    manifest: &WorkspaceTransferManifest,
    missing: &[ReadWorkspaceTransferChunk],
) -> Result<()> {
    let count = manifest
        .files
        .iter()
        .map(|file| file.chunks.len())
        .sum::<usize>();
    if missing.len() > count {
        return Err(Error::Forbidden);
    }
    let mut seen = BTreeSet::new();
    for request in missing {
        declared_chunk(manifest, request)?;
        if !seen.insert((&request.path, request.offset)) {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}

fn validate_chunk(
    manifest: &WorkspaceTransferManifest,
    request: &ReadWorkspaceTransferChunk,
    bytes: &[u8],
) -> Result<()> {
    let chunk = declared_chunk(manifest, request)?;
    if bytes.len() as u64 != chunk.size || hash(bytes) != chunk.sha256 {
        return Err(Error::Forbidden);
    }
    Ok(())
}

fn verify_receipt(
    binding: &Binding,
    request: &Signed<LaunchRequest>,
    receipt: Signed<Receipt>,
) -> Result<Signed<Receipt>> {
    receipt
        .verify("receipt", &binding.runner_public_key)
        .map_err(|_| Error::UncertainEffect)?;
    let value = &receipt.payload;
    if value.request != request.payload
        || value.request_envelope_sha256 != envelope_hash(request)
        || value.updated_at < request.payload.issued_at
    {
        return Err(Error::UncertainEffect);
    }
    match &value.state {
        ExecutionState::Admitted if value.revision == 1 => {}
        ExecutionState::DispatchCommitted if value.revision == 2 => {}
        ExecutionState::Completed { result } if value.revision == 3 => {
            cowork_domain::processes::validate_process_result(result)
                .map_err(|_| Error::UncertainEffect)?;
        }
        _ => return Err(Error::UncertainEffect),
    }
    Ok(receipt)
}

#[cfg(test)]
mod tests;

mod delivery;
