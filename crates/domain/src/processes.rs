use crate::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredProcess {
    pub record: ProcessAttempt,
    /// Original worker fence; never returned as launch authority on a repeated request.
    pub lease_sha256: String,
}

pub fn process_invocation(tool: &ToolCall) -> Result<ProcessInvocation> {
    if tool.name != "execute_program" {
        return Err(Error::Forbidden);
    }
    let call: ProcessInvocation = serde_json::from_value(tool.arguments.clone())
        .map_err(|_| Error::Validation("Invalid process invocation".into()))?;
    valid_text(&tool.id, 128)?;
    valid_text(&call.program, 128)?;
    if call.arguments.len() > 1024
        || call.arguments.iter().any(|v| v.contains('\0'))
        || call.arguments.iter().map(String::len).sum::<usize>() > 32_000
        || !(1..=86_400).contains(&call.timeout_seconds)
    {
        return Err(Error::Validation(
            "Process arguments or time limit are out of range".into(),
        ));
    }
    Ok(call)
}
pub fn process_request_hash(run: &RunRecord, tool: &ToolCall) -> String {
    canonical_hash(&json!({"run_id":run.id,"spec":run.spec,"tool":tool}))
}
pub fn validate_process_result(result: &ProcessResult) -> Result<()> {
    for capture in [&result.stdout, &result.stderr] {
        if capture.bytes.len() > 65_536
            || capture.sha256 != hash(&capture.bytes)
            || capture.total_bytes < capture.bytes.len() as u64
            || capture.truncated != (capture.total_bytes > capture.bytes.len() as u64)
        {
            return Err(Error::Validation("Invalid process output capture".into()));
        }
    }
    if result.process_id == Some(0)
        || (result.process_id.is_none() && (result.exit_code.is_some() || result.signal.is_some()))
        || (result.exit_code.is_some() && result.signal.is_some())
        || result.signal.is_some_and(|signal| signal <= 0)
        || (result.termination == ProcessTermination::Exited
            && (result.process_id.is_none()
                || (result.exit_code.is_none() && result.signal.is_none())))
    {
        return Err(Error::Validation(
            "Invalid process termination evidence".into(),
        ));
    }
    Ok(())
}
pub fn process_result_summary(result: &ProcessResult) -> Value {
    json!({"termination":result.termination,"exit_code":result.exit_code,"signal":result.signal,
        "stdout":String::from_utf8_lossy(&result.stdout.bytes),"stderr":String::from_utf8_lossy(&result.stderr.bytes),
        "stdout_sha256":result.stdout.sha256,"stderr_sha256":result.stderr.sha256,
        "stdout_truncated":result.stdout.truncated,"stderr_truncated":result.stderr.truncated,
        "output_complete":result.output_complete})
}
pub fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
impl StoredProcess {
    pub fn validate(&self) -> Result<()> {
        let p = &self.record;
        process_invocation(&p.tool)?;
        for id in [
            &p.id,
            &p.run_id,
            &p.project_id,
            &p.workspace_resource_id,
            &p.environment.environment_id,
            &p.environment.version_id,
        ] {
            valid_text(id, 128)?;
        }
        if !valid_sha256(&self.lease_sha256)
            || !valid_sha256(&p.request_sha256)
            || !valid_sha256(&p.environment.definition_sha256)
            || !valid_sha256(&p.environment.qualification_sha256)
            || p.created_at <= 0
            || p.updated_at < p.created_at
        {
            return Err(Error::Forbidden);
        }
        match &p.state {
            ProcessAttemptState::LaunchCommitted if p.revision != 1 => {
                return Err(Error::Forbidden);
            }
            ProcessAttemptState::Completed { result } => {
                if p.revision != 2 {
                    return Err(Error::Forbidden);
                }
                validate_process_result(result)?;
            }
            ProcessAttemptState::ReconciledCompleted {
                result,
                evidence_sha256,
            } => {
                if p.revision != 2 || !valid_sha256(evidence_sha256) {
                    return Err(Error::Forbidden);
                }
                validate_process_result(result)?;
            }
            ProcessAttemptState::ReconciledNotStarted { evidence_sha256 } => {
                if p.revision != 2 || !valid_sha256(evidence_sha256) {
                    return Err(Error::Forbidden);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
