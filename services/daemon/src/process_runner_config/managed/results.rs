//! Bounded result content reads on the existing authenticated private origin.
//! A signed completion alone is never accepted instead of a fresh read request.
use super::*;
use cowork_runner_protocol::{
    ManagedResultContextRequest, ManagedResultManifest, ManagedResultReadContext,
    ManagedResultReadOperation, ManagedResultReadRequest,
};

fn read_status(response: &reqwest::Response) -> Result<()> {
    match response.status() {
        reqwest::StatusCode::OK => Ok(()),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => Err(Error::Forbidden),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE => Err(Error::NotFound),
        reqwest::StatusCode::CONFLICT => {
            Err(Error::Validation("managed_result_unavailable".into()))
        }
        _ => Err(Error::UncertainEffect),
    }
}

impl HttpsRunner {
    pub(in crate::process_runner_config) async fn observe_managed_result_context(
        &self,
        request: Signed<ManagedResultContextRequest>,
    ) -> Result<Signed<ManagedResultReadContext>> {
        request
            .verify(
                "managed_result_context_request",
                &self.binding.control_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        request
            .payload
            .validate(
                &self.binding,
                &request.payload.target,
                &request.payload.runtime_instance_sha256,
                now()?,
            )
            .map_err(|_| Error::Forbidden)?;
        if matches!(request.payload.process_attempt_id.as_str(), "." | "..") {
            return Err(Error::Forbidden);
        }
        // Validation above permits only the bounded protocol identifier alphabet.
        let path = format!(
            "v2/managed/results/{}/context",
            request.payload.process_attempt_id
        );
        let started = Instant::now();
        let response = self.managed_post(&path, &request, 65536).await?;
        read_status(&response)?;
        let context: Signed<ManagedResultReadContext> =
            Self::managed_body(response, &[reqwest::StatusCode::OK]).await?;
        context
            .verify("managed_result_context", &self.binding.runner_public_key)
            .map_err(|_| Error::Forbidden)?;
        context
            .payload
            .validate(&request.payload, &self.binding, now()?)
            .map_err(|_| Error::Forbidden)?;
        // Clip from the original call start; neither response nor replay resets it.
        let duration = context
            .payload
            .expires_at
            .checked_sub(request.payload.issued_at)
            .filter(|n| *n > 0)
            .ok_or(Error::Forbidden)?;
        if started.elapsed() >= Duration::from_millis(duration as u64) {
            return Err(Error::Forbidden);
        }
        Ok(context)
    }
    fn result_path(&self, request: &Signed<ManagedResultReadRequest>) -> Result<String> {
        request
            .verify("managed_result_read", &self.binding.control_public_key)
            .map_err(|_| Error::Forbidden)?;
        request
            .payload
            .validate(
                &self.binding,
                &request.payload.target,
                &request.payload.runtime_instance_sha256,
                now()?,
            )
            .map_err(|_| Error::Forbidden)?;
        let id = &request.payload.process_attempt_id;
        if id.is_empty()
            || id.len() > 128
            || matches!(id.as_str(), "." | "..")
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
        {
            return Err(Error::Forbidden);
        }
        Ok(format!("v2/managed/results/{id}/read"))
    }
    pub(in crate::process_runner_config) async fn read_managed_manifest(
        &self,
        request: Signed<ManagedResultReadRequest>,
    ) -> Result<ManagedResultManifest> {
        let path = self.result_path(&request)?;
        if !matches!(
            request.payload.operation,
            ManagedResultReadOperation::Manifest
        ) {
            return Err(Error::Forbidden);
        }
        let started = Instant::now();
        let response = self.managed_post(&path, &request, 65536).await?;
        read_status(&response)?;
        let result: ManagedResultManifest =
            Self::managed_body(response, &[reqwest::StatusCode::OK]).await?;
        result
            .attestation
            .verify("managed_output", &self.binding.runner_public_key)
            .map_err(|_| Error::Forbidden)?;
        workspace_transfer::validate(&result.manifest)?;
        let expected = &request.payload;
        let actual = &result.attestation.payload;
        let observed = now()?;
        if actual.format_version != 1
            || actual.produced_at <= 0
            || observed < actual.produced_at
            || observed >= actual.retained_until
            || actual
                .retained_until
                .checked_sub(actual.produced_at)
                .is_none_or(|n| {
                    n <= 0 || n > cowork_runner_protocol::MANAGED_RESULT_RETENTION_MILLIS
                })
            || !workspace_transfer::valid_sha256(&actual.physical_record_sha256)
            || !workspace_transfer::valid_sha256(&actual.input_manifest_sha256)
            || actual.runtime_instance_sha256 != expected.runtime_instance_sha256
            || actual.execution_plan_id != expected.execution_plan_id
            || actual.execution_plan_sha256 != expected.execution_plan_sha256
            || actual.process_attempt_id != expected.process_attempt_id
            || actual.run_id != expected.run_id
            || actual.launch_envelope_sha256 != expected.launch_envelope_sha256
            || actual.execution_receipt_sha256 != expected.execution_receipt_sha256
            || actual.runner != expected.runner
            || actual.target != expected.target
            || actual.output_manifest_sha256 != result.manifest.sha256
        {
            return Err(Error::Forbidden);
        }
        self.result_path(&request)?;
        if started.elapsed()
            >= Duration::from_millis(
                (request.payload.expires_at - request.payload.issued_at) as u64,
            )
        {
            return Err(Error::Forbidden);
        }
        Ok(result)
    }
    pub(in crate::process_runner_config) async fn read_managed_chunk(
        &self,
        request: Signed<ManagedResultReadRequest>,
        expected_size: u64,
    ) -> Result<Vec<u8>> {
        let path = self.result_path(&request)?;
        let ManagedResultReadOperation::Chunk { chunk, .. } = &request.payload.operation else {
            return Err(Error::Forbidden);
        };
        if expected_size == 0 || expected_size > workspace_transfer::CHUNK_BYTES {
            return Err(Error::Forbidden);
        }
        let expected_hash = chunk.expected_sha256.clone();
        let started = Instant::now();
        let mut response = self.managed_post(&path, &request, 65536).await?;
        read_status(&response)?;
        if response
            .content_length()
            .is_some_and(|size| size != expected_size)
            || response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(';').next())
                != Some("application/octet-stream")
        {
            return Err(Error::FileConflict);
        }
        let mut bytes = Vec::with_capacity(expected_size as usize);
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::UncertainEffect)? {
            if bytes.len().saturating_add(chunk.len()) > expected_size as usize {
                return Err(Error::FileConflict);
            }
            bytes.extend_from_slice(&chunk);
        }
        self.result_path(&request)?;
        if started.elapsed()
            >= Duration::from_millis(
                (request.payload.expires_at - request.payload.issued_at) as u64,
            )
        {
            return Err(Error::Forbidden);
        }
        if bytes.len() as u64 != expected_size || hash(&bytes) != expected_hash {
            return Err(Error::FileConflict);
        }
        Ok(bytes)
    }
}
