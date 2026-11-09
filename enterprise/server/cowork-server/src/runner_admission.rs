//! A capacity response proves no start only for one exact, still-pending request.
//! It never authorizes repeating an agent/tool workflow or an unknown outcome.

use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

const REASON: &str = "sandbox_capacity_exhausted";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedAttempt {
    pub attempt_id: uuid::Uuid,
    pub lease_token: uuid::Uuid,
    pub request_sha256: String,
    pub workspace_id: Option<uuid::Uuid>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StopRequest {
    pub version: u8,
    pub run_id: uuid::Uuid,
    pub lease_token: Option<uuid::Uuid>,
    pub cancel_lease: Option<uuid::Uuid>,
    pub attempts: Vec<ExpectedAttempt>,
}

/// Only a request-bound, authenticated physical stop can construct this type.
#[derive(Debug)]
pub(crate) struct VerifiedStop {
    request: StopRequest,
    receipt: serde_json::Value,
}

impl VerifiedStop {
    pub(crate) fn matches(
        &self,
        run_id: uuid::Uuid,
        lease_token: Option<uuid::Uuid>,
        cancel_lease: Option<uuid::Uuid>,
        attempts: &[ExpectedAttempt],
    ) -> bool {
        self.request.run_id == run_id
            && self.request.lease_token == lease_token
            && self.request.cancel_lease == cancel_lease
            && self.request.attempts == attempts
    }
    pub(crate) fn receipt(&self) -> &serde_json::Value {
        &self.receipt
    }
}

pub(crate) fn execution_body(
    spec: &cowork_contracts::SandboxRunSpec,
    lease_token: uuid::Uuid,
    attempt_id: uuid::Uuid,
) -> anyhow::Result<Vec<u8>> {
    let mut body = serde_json::to_value(spec)?;
    body["_execution"] =
        serde_json::json!({"version":1,"lease_token":lease_token,"attempt_id":attempt_id});
    Ok(serde_json::to_vec(&body)?)
}

pub(crate) struct RequestAttempt {
    method: String,
    path_and_query: String,
    timestamp: String,
    nonce: String,
    digest: String,
    consumed: bool,
}

impl RequestAttempt {
    pub(crate) fn new(
        method: &str,
        path_and_query: &str,
        timestamp: &str,
        nonce: &str,
        body: &[u8],
    ) -> Self {
        Self {
            method: method.to_owned(),
            path_and_query: path_and_query.to_owned(),
            timestamp: timestamp.to_owned(),
            nonce: nonce.to_owned(),
            digest: hex::encode(Sha256::digest(body)),
            consumed: false,
        }
    }

    pub(crate) fn verify_stop(
        &mut self,
        key: &[u8],
        status: u16,
        headers: &HeaderMap,
        response: &[u8],
        request: &StopRequest,
    ) -> Option<VerifiedStop> {
        if self.consumed {
            return None;
        }
        self.consumed = true;
        if status != 200 || response.len() > 32 * 1024 * 1024 {
            return None;
        }
        let proof_values = headers.get_all("x-cowork-stop-proof");
        let mut values = proof_values.iter();
        let value = values.next()?.to_str().ok()?;
        if values.next().is_some() {
            return None;
        }
        let signature = hex::decode(value.strip_prefix("v1:")?).ok()?;
        if signature.len() != 32 {
            return None;
        }
        let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
        mac.update(b"open-cowork/runner-stop/v1\0");
        let response_digest = hex::encode(Sha256::digest(response));
        for field in [
            self.method.as_str(),
            self.path_and_query.as_str(),
            self.timestamp.as_str(),
            self.nonce.as_str(),
            self.digest.as_str(),
            "200",
            response_digest.as_str(),
        ] {
            mac.update(&(field.len() as u64).to_be_bytes());
            mac.update(field.as_bytes());
        }
        mac.verify_slice(&signature).ok()?;
        let receipt: serde_json::Value = serde_json::from_slice(response).ok()?;
        if request.version != 1
            || receipt["version"] != 1
            || receipt["run_id"] != request.run_id.to_string()
            || receipt["lease_token"] != serde_json::to_value(request.lease_token).ok()?
            || receipt["cancel_lease"] != serde_json::to_value(request.cancel_lease).ok()?
            || receipt["attempts"] != serde_json::to_value(&request.attempts).ok()?
            || receipt["execution_stopped"] != true
        {
            return None;
        }
        let mut ids = std::collections::HashSet::new();
        if receipt["backend"] == "kubernetes" {
            if !receipt["containers"].as_array()?.is_empty() {
                return None;
            }
            let namespace = uuid::Uuid::parse_str(receipt["namespace_uid"].as_str()?).ok()?;
            let cluster = uuid::Uuid::parse_str(receipt["cluster_uid"].as_str()?).ok()?;
            if namespace.is_nil() || cluster.is_nil() {
                return None;
            }
            let mut pods = std::collections::HashSet::new();
            for row in receipt["physical_pods"].as_array()? {
                let uid = uuid::Uuid::parse_str(row["pod_uid"].as_str()?).ok()?;
                let attempt = uuid::Uuid::parse_str(row["attempt_id"].as_str()?).ok()?;
                if uid.is_nil()
                    || !pods.insert(uid)
                    || !request.attempts.iter().any(|a| a.attempt_id == attempt)
                    || row["proof"]["pod_uid"] != row["pod_uid"]
                    || row["proof"]["namespace_uid"] != receipt["namespace_uid"]
                    || row["proof"]["cluster_uid"] != receipt["cluster_uid"]
                {
                    return None;
                }
                let states = row["proof"]["container_statuses"].as_array()?;
                if states.is_empty()
                    || states.iter().any(|s| {
                        !s["state"]["terminated"]["finishedAt"].is_string()
                            || !s["containerID"].is_string()
                    })
                {
                    return None;
                }
            }
            for field in ["physical_transfers", "physical_desktops"] {
                for row in receipt[field].as_array()? {
                    let proof = if field == "physical_transfers" {
                        &row["proof"]
                    } else {
                        row
                    };
                    let uid = uuid::Uuid::parse_str(proof["pod_uid"].as_str()?).ok()?;
                    if uid.is_nil()
                        || proof["namespace_uid"] != receipt["namespace_uid"]
                        || proof["cluster_uid"] != receipt["cluster_uid"]
                    {
                        return None;
                    }
                    if field == "physical_transfers"
                        && (row["pod_uid"] != proof["pod_uid"]
                            || uuid::Uuid::parse_str(row["request_id"].as_str()?)
                                .ok()?
                                .is_nil())
                    {
                        return None;
                    }
                    let states = proof["container_statuses"].as_array()?;
                    if states.is_empty()
                        || states.iter().any(|s| {
                            !s["state"]["terminated"]["finishedAt"].is_string()
                                || !s["containerID"]
                                    .as_str()
                                    .is_some_and(|id| id.contains("://"))
                        })
                    {
                        return None;
                    }
                }
            }
        }
        for container in receipt["containers"].as_array()? {
            let id = container["container_id"].as_str()?;
            if id.len() != 64 || !id.chars().all(|ch| ch.is_ascii_hexdigit()) || !ids.insert(id) {
                return None;
            }
            container["container_name"].as_str()?;
            uuid::Uuid::parse_str(container["reservation_id"].as_str()?).ok()?;
        }
        Some(VerifiedStop {
            request: request.clone(),
            receipt,
        })
    }

    /// Consume the decision even when invalid: a later substitute response
    /// cannot retroactively turn an unknown outcome into retry authority.
    pub(crate) fn verify_capacity_no_start(
        &mut self,
        key: &[u8],
        status: u16,
        headers: &HeaderMap,
        response_body: &[u8],
    ) -> bool {
        if self.consumed {
            return false;
        }
        self.consumed = true;
        if status != 429 || response_body.len() > 16 * 1024 {
            return false;
        }
        let Ok(body) = serde_json::from_slice::<serde_json::Value>(response_body) else {
            return false;
        };
        if body.get("error").and_then(serde_json::Value::as_str) != Some(REASON) {
            return false;
        }
        let proof_values = headers.get_all("x-cowork-no-start-proof");
        let mut values = proof_values.iter();
        let Some(value) = values.next().and_then(|value| value.to_str().ok()) else {
            return false;
        };
        if values.next().is_some() {
            return false;
        }
        let Some(signature) = value.strip_prefix("v1:") else {
            return false;
        };
        if signature.len() != 64 {
            return false;
        }
        let Ok(signature) = hex::decode(signature) else {
            return false;
        };
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
            return false;
        };
        mac.update(b"open-cowork/runner-no-start/v1\0");
        for field in [
            self.method.as_str(),
            self.path_and_query.as_str(),
            self.timestamp.as_str(),
            self.nonce.as_str(),
            self.digest.as_str(),
            "429",
            REASON,
        ] {
            mac.update(&(field.len() as u64).to_be_bytes());
            mac.update(field.as_bytes());
        }
        mac.verify_slice(&signature).is_ok()
    }
}

#[cfg(test)]
pub(crate) fn verified_stop_for_test(request: &StopRequest) -> VerifiedStop {
    let body = serde_json::to_vec(request).unwrap();
    let response = serde_json::to_vec(&serde_json::json!({"version":1,"run_id":request.run_id,"lease_token":request.lease_token,"cancel_lease":request.cancel_lease,"attempts":request.attempts,"containers":[],"execution_stopped":true})).unwrap();
    let mut pending = RequestAttempt::new(
        "POST",
        "/v1/execution-stop",
        "1700000000",
        "fixture-nonce",
        &body,
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(&[b'k'; 32]).unwrap();
    mac.update(b"open-cowork/runner-stop/v1\0");
    let digest = hex::encode(Sha256::digest(&response));
    for field in [
        pending.method.as_str(),
        pending.path_and_query.as_str(),
        pending.timestamp.as_str(),
        pending.nonce.as_str(),
        pending.digest.as_str(),
        "200",
        digest.as_str(),
    ] {
        mac.update(&(field.len() as u64).to_be_bytes());
        mac.update(field.as_bytes());
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-cowork-stop-proof",
        format!("v1:{}", hex::encode(mac.finalize().into_bytes()))
            .parse()
            .unwrap(),
    );
    pending
        .verify_stop(&[b'k'; 32], 200, &headers, &response, request)
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: &str = "00000000-0000-4000-8000-000000000001";
    const REQUEST: &[u8] = br#"{"run_id":"fixture"}"#;
    const RESPONSE: &[u8] = br#"{"error":"sandbox_capacity_exhausted","message":"capacity"}"#;
    // Shared fixed vector with the runner test; independent of test-local signing.
    const PROOF: &str = "v1:a97929d8a4c4b5a99db42ca778a568afcbdca1951c81878dcca50d7324262349";
    const STOP_REQUEST: &[u8] = br#"{"version":1,"run_id":"00000000-0000-4000-8000-000000000002","lease_token":null,"cancel_lease":null,"attempts":[]}"#;
    const STOP_RESPONSE: &[u8] = br#"{"version":1,"run_id":"00000000-0000-4000-8000-000000000002","lease_token":null,"cancel_lease":null,"attempts":[],"containers":[],"execution_stopped":true}"#;
    const STOP_PROOF: &str = "v1:4d5ee56fc94d759dd2261b87b79cf41dbbfa08b262b4fa2e7f5c88ccd520393a";

    fn attempt() -> RequestAttempt {
        RequestAttempt::new("POST", "/v1/jobs", "1700000000", NONCE, REQUEST)
    }

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-cowork-no-start-proof", value.parse().unwrap());
        headers
    }

    #[test]
    fn authentic_no_start_is_bound_to_one_request_and_consumed_once() {
        let mut pending = attempt();
        assert!(pending.verify_capacity_no_start(&[b'k'; 32], 429, &headers(PROOF), RESPONSE));
        assert!(!pending.verify_capacity_no_start(&[b'k'; 32], 429, &headers(PROOF), RESPONSE));
        for mut changed in [
            RequestAttempt::new("GET", "/v1/jobs", "1700000000", NONCE, REQUEST),
            RequestAttempt::new("POST", "/v2/jobs", "1700000000", NONCE, REQUEST),
            RequestAttempt::new("POST", "/v1/jobs?other=1", "1700000000", NONCE, REQUEST),
            RequestAttempt::new("POST", "/v1/jobs", "1700000001", NONCE, REQUEST),
            RequestAttempt::new("POST", "/v1/jobs", "1700000000", "new-nonce", REQUEST),
            RequestAttempt::new("POST", "/v1/jobs", "1700000000", NONCE, b"changed"),
        ] {
            assert!(!changed.verify_capacity_no_start(&[b'k'; 32], 429, &headers(PROOF), RESPONSE));
        }
    }

    #[test]
    fn generic_forged_duplicate_and_conflict_responses_never_prove_no_start() {
        for proof in ["", "v2:abcd", "v1:not-hex", "v1:00"] {
            assert!(!attempt().verify_capacity_no_start(
                &[b'k'; 32],
                429,
                &headers(proof),
                RESPONSE
            ));
        }
        assert!(!attempt().verify_capacity_no_start(&[b'x'; 32], 429, &headers(PROOF), RESPONSE));
        assert!(!attempt().verify_capacity_no_start(&[b'k'; 32], 409, &headers(PROOF), RESPONSE));
        assert!(!attempt().verify_capacity_no_start(&[b'k'; 32], 200, &headers(PROOF), RESPONSE));
        assert!(!attempt().verify_capacity_no_start(&[b'k'; 32], 429, &HeaderMap::new(), RESPONSE));
        assert!(!attempt().verify_capacity_no_start(
            &[b'k'; 32],
            429,
            &headers(PROOF),
            b"rate limit"
        ));
        assert!(!attempt().verify_capacity_no_start(
            &[b'k'; 32],
            429,
            &headers(PROOF),
            br#"{"error":"sandbox_launch_unproven"}"#,
        ));
        let mut duplicate = headers(PROOF);
        duplicate.append("x-cowork-no-start-proof", PROOF.parse().unwrap());
        assert!(!attempt().verify_capacity_no_start(&[b'k'; 32], 429, &duplicate, RESPONSE));
        let mut consumed = attempt();
        assert!(!consumed.verify_capacity_no_start(&[b'k'; 32], 429, &HeaderMap::new(), RESPONSE));
        assert!(!consumed.verify_capacity_no_start(&[b'k'; 32], 429, &headers(PROOF), RESPONSE));
    }

    #[test]
    fn physical_stop_proof_covers_exact_run_lease_and_attempt_set() {
        let request = StopRequest {
            version: 1,
            run_id: uuid::Uuid::new_v4(),
            lease_token: None,
            cancel_lease: Some(uuid::Uuid::new_v4()),
            attempts: vec![ExpectedAttempt {
                attempt_id: uuid::Uuid::new_v4(),
                lease_token: uuid::Uuid::new_v4(),
                request_sha256: "a".repeat(64),
                workspace_id: Some(uuid::Uuid::new_v4()),
            }],
        };
        let verified = verified_stop_for_test(&request);
        assert!(verified.matches(
            request.run_id,
            None,
            request.cancel_lease,
            &request.attempts
        ));
        assert!(!verified.matches(
            uuid::Uuid::new_v4(),
            None,
            request.cancel_lease,
            &request.attempts
        ));
        assert!(!verified.matches(
            request.run_id,
            request.cancel_lease,
            request.cancel_lease,
            &request.attempts
        ));
        assert!(!verified.matches(request.run_id, None, None, &request.attempts));
        assert!(!verified.matches(request.run_id, None, request.cancel_lease, &[]));
        let mut changed = request.attempts.clone();
        changed[0].request_sha256 = "b".repeat(64);
        assert!(!verified.matches(request.run_id, None, request.cancel_lease, &changed));
    }

    #[test]
    fn stop_response_cannot_reuse_no_start_or_request_authentication_domains() {
        let request = StopRequest {
            version: 1,
            run_id: uuid::Uuid::new_v4(),
            lease_token: None,
            cancel_lease: None,
            attempts: vec![],
        };
        let body = serde_json::to_vec(&request).unwrap();
        let response = serde_json::to_vec(verified_stop_for_test(&request).receipt()).unwrap();
        let mut headers = headers(PROOF);
        headers.insert("x-cowork-stop-proof", PROOF.parse().unwrap());
        let mut pending =
            RequestAttempt::new("POST", "/v1/execution-stop", "1700000000", NONCE, &body);
        assert!(pending
            .verify_stop(&[b'k'; 32], 200, &headers, &response, &request)
            .is_none());
        // Even after replacing a forged response, this request is consumed.
        assert!(pending
            .verify_stop(&[b'k'; 32], 200, &HeaderMap::new(), &response, &request)
            .is_none());
    }

    #[test]
    fn stop_fixed_vector_rejects_tampering_wrong_scope_replay_and_duplicate_headers() {
        let request: StopRequest = serde_json::from_slice(STOP_REQUEST).unwrap();
        let mut proof_headers = HeaderMap::new();
        proof_headers.insert("x-cowork-stop-proof", STOP_PROOF.parse().unwrap());
        let fresh = || {
            RequestAttempt::new(
                "POST",
                "/v1/execution-stop",
                "1700000000",
                NONCE,
                STOP_REQUEST,
            )
        };
        let mut pending = fresh();
        assert!(pending
            .verify_stop(&[b'k'; 32], 200, &proof_headers, STOP_RESPONSE, &request)
            .is_some());
        assert!(pending
            .verify_stop(&[b'k'; 32], 200, &proof_headers, STOP_RESPONSE, &request)
            .is_none());
        for mut changed in [
            RequestAttempt::new(
                "DELETE",
                "/v1/execution-stop",
                "1700000000",
                NONCE,
                STOP_REQUEST,
            ),
            RequestAttempt::new(
                "POST",
                "/v1/execution-stop?scope=other",
                "1700000000",
                NONCE,
                STOP_REQUEST,
            ),
            RequestAttempt::new(
                "POST",
                "/v1/execution-stop",
                "1700000001",
                NONCE,
                STOP_REQUEST,
            ),
            RequestAttempt::new(
                "POST",
                "/v1/execution-stop",
                "1700000000",
                "different",
                STOP_REQUEST,
            ),
            RequestAttempt::new(
                "POST",
                "/v1/execution-stop",
                "1700000000",
                NONCE,
                b"changed",
            ),
        ] {
            assert!(changed
                .verify_stop(&[b'k'; 32], 200, &proof_headers, STOP_RESPONSE, &request)
                .is_none());
        }
        assert!(fresh()
            .verify_stop(&[b'x'; 32], 200, &proof_headers, STOP_RESPONSE, &request)
            .is_none());
        assert!(fresh()
            .verify_stop(&[b'k'; 32], 409, &proof_headers, STOP_RESPONSE, &request)
            .is_none());
        assert!(fresh()
            .verify_stop(&[b'k'; 32], 200, &proof_headers, b"changed", &request)
            .is_none());
        proof_headers.append("x-cowork-stop-proof", STOP_PROOF.parse().unwrap());
        assert!(fresh()
            .verify_stop(&[b'k'; 32], 200, &proof_headers, STOP_RESPONSE, &request)
            .is_none());
    }
}
