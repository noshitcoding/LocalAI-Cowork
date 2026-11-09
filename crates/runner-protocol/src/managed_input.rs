//! Portable private controller/runner contract. Signatures and current local
//! runner/target binding are checked independently of any transfer journal.
use super::*;
use cowork_contracts::{ExecutionRunnerIdentity, ExecutionTargetRef};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedInputGrant {
    pub format_version: u32,
    pub execution_plan_id: String,
    pub execution_plan_sha256: String,
    pub target: ExecutionTargetRef,
    pub runner: ExecutionRunnerIdentity,
    pub input_manifest_sha256: String,
    pub process_attempt_id: String,
    pub run_id: String,
    pub request_sha256: String,
    pub environment: ExecutionEnvironmentRef,
    pub invocation_sha256: String,
    pub worker_fence_sha256: String,
    pub lease_epoch: u64,
    pub issued_at: i64,
    pub expires_at: i64,
}

impl ManagedInputGrant {
    /// Shape, time and expected installed recipient checks; this does not verify
    /// a signature or create launch/transfer authority by itself.
    pub fn validate_for_runner(
        &self,
        binding: &Binding,
        current_target: &ExecutionTargetRef,
        now: i64,
    ) -> Result<()> {
        binding.validate()?;
        let grant = self;
        let identity = ExecutionRunnerIdentity {
            runner_id: binding.runner_id.clone(),
            generation: binding.generation,
            adapter_build_sha256: binding.adapter_build_sha256.clone(),
            control_public_key: binding.control_public_key.clone(),
            runner_public_key: binding.runner_public_key.clone(),
        };
        if grant.format_version != 1
            || grant.runner != identity
            || grant.target != *current_target
            || current_target.revision <= 0
            || current_target.target_id
                != cowork_domain::execution_targets::execution_target_id(&identity)
                    .map_err(|_| Error::Authority)?
            || current_target.identity_sha256
                != cowork_domain::execution_targets::runner_identity_hash(&identity)
                    .map_err(|_| Error::Authority)?
            || [
                &grant.execution_plan_id,
                &grant.process_attempt_id,
                &grant.run_id,
            ]
            .iter()
            .any(|v| {
                v.is_empty()
                    || v.len() > 128
                    || !v
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
            })
            || [
                &grant.execution_plan_sha256,
                &grant.input_manifest_sha256,
                &grant.request_sha256,
                &grant.invocation_sha256,
                &grant.worker_fence_sha256,
                &grant.environment.definition_sha256,
                &grant.environment.qualification_sha256,
            ]
            .iter()
            .any(|v| !valid_sha256(v))
            || grant.environment.environment_id.is_empty()
            || grant.environment.environment_id.len() > 128
            || grant.environment.version_id.is_empty()
            || grant.environment.version_id.len() > 128
            || grant.lease_epoch == 0
            || grant.lease_epoch > i64::MAX as u64
            || grant.issued_at <= 0
            || now < grant.issued_at
            || now >= grant.expires_at
            || grant
                .expires_at
                .checked_sub(grant.issued_at)
                .is_none_or(|v| v <= 0 || v > 300000)
        {
            return Err(Error::Authority);
        }
        Ok(())
    }
    /// A path/resource identifier never substitutes for this exact intent.
    pub fn validate_for_launch(
        &self,
        binding: &Binding,
        target: &ExecutionTargetRef,
        request: &LaunchRequest,
        now: i64,
    ) -> Result<()> {
        self.validate_for_runner(binding, target, now)?;
        request.validate(binding)?;
        request.assert_fresh(now)?;
        if self.runner.runner_id != request.runner_id
            || self.runner.generation != request.runner_generation
            || self.runner.adapter_build_sha256 != request.adapter_build_sha256
            || self.process_attempt_id != request.process_attempt_id
            || self.run_id != request.run_id
            || self.request_sha256 != request.request_sha256
            || self.environment != request.environment
            || self.invocation_sha256 != canonical_hash(&request.invocation)
            || self.worker_fence_sha256 != request.worker_fence_sha256
            || self.lease_epoch != request.lease_epoch
        {
            return Err(Error::Authority);
        }
        Ok(())
    }
    /// No independent renewal protocol is supported yet. All initially signed
    /// expirations remain hard physical limits; narrower input rights stay narrow.
    pub fn initial_execution_deadline(&self, request: &LaunchRequest) -> i64 {
        self.expires_at
            .min(request.expires_at)
            .min(request.execution_deadline)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn fixture() -> (
        Binding,
        ExecutionTargetRef,
        ManagedInputGrant,
        LaunchRequest,
        SigningKey,
    ) {
        let control = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
        let runner = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
        let binding = Binding {
            runner_id: "portable-input-recipient".into(),
            generation: 7,
            adapter_build_sha256: "a".repeat(64),
            control_public_key: control.public_key(),
            runner_public_key: runner.public_key(),
        };
        let identity = ExecutionRunnerIdentity {
            runner_id: binding.runner_id.clone(),
            generation: binding.generation,
            adapter_build_sha256: binding.adapter_build_sha256.clone(),
            control_public_key: binding.control_public_key.clone(),
            runner_public_key: binding.runner_public_key.clone(),
        };
        let target = ExecutionTargetRef {
            target_id: cowork_domain::execution_targets::execution_target_id(&identity).unwrap(),
            revision: 9,
            identity_sha256: cowork_domain::execution_targets::runner_identity_hash(&identity)
                .unwrap(),
        };
        let request = LaunchRequest {
            format_version: 1,
            runner_id: binding.runner_id.clone(),
            runner_generation: binding.generation,
            adapter_build_sha256: binding.adapter_build_sha256.clone(),
            process_attempt_id: "attempt-once".into(),
            run_id: "run-once".into(),
            tool_call_id: "tool-once".into(),
            request_sha256: "b".repeat(64),
            worker_fence_sha256: "c".repeat(64),
            lease_epoch: 2,
            environment: ExecutionEnvironmentRef {
                environment_id: "env".into(),
                version_id: "version".into(),
                definition_sha256: "d".repeat(64),
                qualification_sha256: "e".repeat(64),
            },
            workspace_resource_id: "this-label-is-not-input-authority".into(),
            invocation: cowork_contracts::ProcessInvocation {
                program: "probe".into(),
                arguments: vec!["copy".into()],
                timeout_seconds: 5,
            },
            issued_at: 1000,
            expires_at: 21000,
            execution_deadline: 61000,
        };
        let grant = ManagedInputGrant {
            format_version: 1,
            execution_plan_id: "confirmed-plan".into(),
            execution_plan_sha256: "f".repeat(64),
            target: target.clone(),
            runner: identity,
            input_manifest_sha256: "1".repeat(64),
            process_attempt_id: request.process_attempt_id.clone(),
            run_id: request.run_id.clone(),
            request_sha256: request.request_sha256.clone(),
            environment: request.environment.clone(),
            invocation_sha256: canonical_hash(&request.invocation),
            worker_fence_sha256: request.worker_fence_sha256.clone(),
            lease_epoch: request.lease_epoch,
            issued_at: 1000,
            expires_at: 31000,
        };
        (binding, target, grant, request, control)
    }
    #[test]
    fn portable_input_signature_preserves_purpose_and_rejects_unknown_fields() {
        let (binding, target, grant, request, control) = fixture();
        let signed = control.sign("managed_input_grant", grant.clone()).unwrap();
        signed
            .verify("managed_input_grant", &binding.control_public_key)
            .unwrap();
        signed
            .payload
            .validate_for_launch(&binding, &target, &request, 2000)
            .unwrap();
        assert!(
            signed
                .verify("launch", &binding.control_public_key)
                .is_err()
        );
        let wrong = control.sign("launch", grant.clone()).unwrap();
        assert!(
            wrong
                .verify("managed_input_grant", &binding.control_public_key)
                .is_err()
        );
        let mut json = serde_json::to_value(grant).unwrap();
        json["host_path"] = serde_json::json!("/home/original");
        assert!(serde_json::from_value::<ManagedInputGrant>(json).is_err());
    }
    #[test]
    fn portable_input_rejects_changed_recipient_and_every_bound_launch_intent() {
        let (binding, target, grant, request, _) = fixture();
        let changes: [fn(&mut LaunchRequest); 9] = [
            |r| r.process_attempt_id.push('x'),
            |r| r.run_id.push('x'),
            |r| r.request_sha256 = "2".repeat(64),
            |r| r.worker_fence_sha256 = "3".repeat(64),
            |r| r.lease_epoch += 1,
            |r| r.runner_generation += 1,
            |r| r.adapter_build_sha256 = "4".repeat(64),
            |r| r.environment.qualification_sha256 = "5".repeat(64),
            |r| r.invocation.arguments.push("unconfirmed".into()),
        ];
        for change in changes {
            let mut altered = request.clone();
            change(&mut altered);
            assert!(
                grant
                    .validate_for_launch(&binding, &target, &altered, 2000)
                    .is_err()
            );
        }
        let mut target = target.clone();
        target.revision += 1;
        assert!(
            grant
                .validate_for_launch(&binding, &target, &request, 2000)
                .is_err()
        );
        let mut binding = binding;
        binding.control_public_key[0] ^= 1;
        assert!(
            grant
                .validate_for_runner(&binding, &grant.target, 2000)
                .is_err()
        );
    }
    #[test]
    fn input_expiry_and_initial_launch_lease_cannot_extend_each_other() {
        let (binding, target, mut grant, request, _) = fixture();
        assert_eq!(
            grant.initial_execution_deadline(&request),
            request.expires_at
        );
        assert!(
            grant
                .validate_for_launch(&binding, &target, &request, request.expires_at)
                .is_err()
        );
        grant.expires_at = 1500;
        assert_eq!(grant.initial_execution_deadline(&request), 1500);
        assert!(
            grant
                .validate_for_launch(&binding, &target, &request, 1500)
                .is_err()
        );
        grant
            .validate_for_launch(&binding, &target, &request, 1499)
            .unwrap();
    }
}
