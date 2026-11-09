//! Registry identity is derived from actual controller key + runner ID. Changing
//! generation, runner key or build requires an inactive new registration revision.
use crate::*;

pub fn validate_runner_identity(identity: &ExecutionRunnerIdentity) -> Result<()> {
    if identity.runner_id.is_empty()
        || identity.runner_id.len() > 128
        || !identity
            .runner_id
            .bytes()
            .all(|byte| byte.is_ascii_graphic())
        || identity.generation == 0
        || identity.generation > i64::MAX as u64
        || !processes::valid_sha256(&identity.adapter_build_sha256)
        || identity.control_public_key.len() != 32
        || identity.runner_public_key.len() != 32
        || identity.control_public_key.iter().all(|byte| *byte == 0)
        || identity.runner_public_key.iter().all(|byte| *byte == 0)
        || identity.control_public_key == identity.runner_public_key
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

pub fn execution_target_id(identity: &ExecutionRunnerIdentity) -> Result<String> {
    validate_runner_identity(identity)?;
    Ok(canonical_hash(&json!({"domain":"execution-target-id/1",
        "control_public_key":identity.control_public_key,"runner_id":identity.runner_id})))
}

pub fn runner_identity_hash(identity: &ExecutionRunnerIdentity) -> Result<String> {
    validate_runner_identity(identity)?;
    Ok(canonical_hash(
        &json!({"domain":"execution-runner-identity/1","identity":identity}),
    ))
}

pub fn validate_target(target: &ExecutionTarget) -> Result<()> {
    if target.format_version != 1
        || target.target_id != execution_target_id(&target.identity)?
        || target.identity_sha256 != runner_identity_hash(&target.identity)?
        || target.revision <= 0
        || target.created_at <= 0
        || target.updated_at < target.created_at
    {
        return Err(Error::Conflict);
    }
    Ok(())
}

pub fn target_reference(target: &ExecutionTarget) -> Result<ExecutionTargetRef> {
    validate_target(target)?;
    if target.state != ExecutionTargetState::Active {
        return Err(Error::Forbidden);
    }
    Ok(ExecutionTargetRef {
        target_id: target.target_id.clone(),
        revision: target.revision,
        identity_sha256: target.identity_sha256.clone(),
    })
}
