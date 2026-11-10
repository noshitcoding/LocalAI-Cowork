use crate::{config::Configuration, workspace};
use anyhow::{Context, Result, ensure};
use cowork_contracts::{ProcessCapture, ProcessInvocation, ProcessResult, ProcessTermination};
use cowork_domain::environments::{EnvironmentDefinition, IsolationCapability};
use cowork_executor::{Outcome, TrustedLaunch};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
#[cfg(target_os = "linux")]
#[path = "platform_linux.rs"]
mod implementation;
#[cfg(windows)]
#[path = "platform_windows.rs"]
mod implementation;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
pub fn result(outcome: Outcome) -> ProcessResult {
    fn capture(c: cowork_executor::Capture) -> ProcessCapture {
        ProcessCapture {
            sha256: workspace::digest(&c.bytes),
            bytes: c.bytes,
            total_bytes: c.total_bytes,
            truncated: c.truncated,
        }
    }
    ProcessResult {
        termination: match outcome.termination {
            cowork_executor::Termination::Exited => ProcessTermination::Exited,
            cowork_executor::Termination::Canceled => ProcessTermination::Canceled,
            cowork_executor::Termination::TimedOut => ProcessTermination::TimedOut,
        },
        process_id: outcome.process_id,
        exit_code: outcome.exit_code,
        signal: outcome.signal,
        stdout: capture(outcome.stdout),
        stderr: capture(outcome.stderr),
        duration_ms: outcome.duration_ms,
        output_complete: outcome.output_complete,
    }
}
pub fn trusted(
    program: &Path,
    directory: &Path,
    args: Vec<String>,
    timeout: Duration,
) -> Result<TrustedLaunch> {
    crate::config::ordinary(program, false)?;
    crate::config::ordinary(directory, true)?;
    let mut environment = BTreeMap::new();
    #[cfg(windows)]
    {
        let root = std::env::var("SystemRoot").context("SystemRoot missing")?;
        ensure!(Path::new(&root).is_absolute(), "Invalid SystemRoot");
        environment.insert("SystemRoot".into(), root);
    }
    #[cfg(target_os = "linux")]
    environment.insert("PATH".into(), "/usr/sbin:/usr/bin:/sbin:/bin".into());
    Ok(TrustedLaunch {
        program: program.to_path_buf(),
        directory: directory.to_path_buf(),
        arguments: args,
        environment,
        timeout,
        capture_bytes_per_stream: 65536,
    })
}
pub fn command(program: &Path, directory: &Path, args: Vec<String>) -> Result<Outcome> {
    let outcome = cowork_executor::supervise(
        &trusted(program, directory, args, Duration::from_secs(30))?,
        &AtomicBool::new(false),
    )?;
    ensure!(
        outcome.termination == cowork_executor::Termination::Exited
            && outcome.exit_code == Some(0)
            && outcome.output_complete
            && !outcome.stdout.truncated,
        "Operator adapter command failed"
    );
    Ok(outcome)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ownership {
    pub nonce: String,
    pub adapter: String,
    pub physical_id: Option<String>,
    #[serde(default)]
    pub mount_volume: Option<String>,
    pub artifact: String,
    pub definition_sha256: String,
    pub owned_directory: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_sandbox: Option<crate::agent_sandbox::RunOwnership>,
}
pub fn inspect(config: &Configuration, definition: &EnvironmentDefinition) -> Result<()> {
    definition.validate()?;
    implementation::inspect(config, definition)
}
pub fn recover(config: &Configuration, record: &Ownership) -> Result<()> {
    implementation::recover(config, record)
}
pub fn release(config: &Configuration, record: &Ownership) -> Result<()> {
    implementation::release(config, record)
}
#[cfg(windows)]
pub fn inspect_orphan(
    config: &Configuration,
    record: &Ownership,
    root: &Path,
) -> Result<cowork_executor::windows_isolation::OrphanInspection> {
    implementation::inspect_orphan(config, record, root)
}
#[cfg(windows)]
pub fn validate_orphan_mount(
    config: &Configuration,
    record: &Ownership,
    root: &Path,
) -> Result<()> {
    implementation::validate_orphan_mount(config, record, root)
}
pub fn run(
    config: &Configuration,
    definition: &EnvironmentDefinition,
    invocation: &ProcessInvocation,
    input: &Path,
    attempt: &Path,
    cancel: &AtomicBool,
    probe: bool,
) -> Result<(Outcome, PathBuf)> {
    inspect(config, definition)?;
    implementation::run(
        config, definition, invocation, input, attempt, cancel, probe,
    )
}
pub fn run_workspace(
    config: &Configuration,
    definition: &EnvironmentDefinition,
    invocation: &ProcessInvocation,
    input: &Path,
    attempt: &Path,
    cancel: &AtomicBool,
    workspace_resource_id: &str,
) -> Result<(Outcome, PathBuf)> {
    if matches!(
        definition.artifact,
        cowork_domain::environments::EnvironmentArtifact::LinuxKataImage { .. }
    ) {
        inspect(config, definition)?;
        return crate::agent_sandbox::run(
            config,
            definition,
            invocation,
            crate::agent_sandbox::RunPaths { input, attempt },
            cancel,
            false,
            Some(workspace_resource_id),
        );
    }
    run(
        config, definition, invocation, input, attempt, cancel, false,
    )
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    read_denied: bool,
    write_denied: bool,
    network_denied: bool,
    credentials_isolated: bool,
    child_started: bool,
    workspace_writable: bool,
    #[serde(default)]
    resource_limits_enforced: Option<bool>,
}
pub fn qualify(
    config: &Configuration,
    definition: &EnvironmentDefinition,
    attempt: &Path,
) -> Result<(String, BTreeSet<IsolationCapability>)> {
    inspect(config, definition)?;
    let input = attempt.join("input");
    crate::config::private_directory(&input)?;
    let sentinel = attempt.join("host-sentinel.txt");
    crate::config::write_new(&sentinel, b"private qualification sentinel")?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let mut probe = ProcessInvocation {
        program: "__qualification".into(),
        arguments: vec![
            sentinel.to_string_lossy().into_owned(),
            listener.local_addr()?.to_string(),
            ".".into(),
        ],
        timeout_seconds: 20,
    };
    if matches!(
        definition.artifact,
        cowork_domain::environments::EnvironmentArtifact::LinuxImage { .. }
            | cowork_domain::environments::EnvironmentArtifact::LocalLinuxImage { .. }
    ) {
        probe.arguments.extend([
            definition.limits.memory_bytes.to_string(),
            definition.limits.cpu_millis.to_string(),
            definition.limits.processes.to_string(),
        ]);
    }
    let (outcome, output) = run(
        config,
        definition,
        &probe,
        &input,
        attempt,
        &AtomicBool::new(false),
        true,
    )?;
    ensure!(
        outcome.termination == cowork_executor::Termination::Exited
            && outcome.exit_code == Some(0)
            && outcome.output_complete,
        "Isolation qualification probe failed"
    );
    let report: Probe = serde_json::from_slice(&outcome.stdout.bytes)?;
    if matches!(
        definition.artifact,
        cowork_domain::environments::EnvironmentArtifact::LinuxImage { .. }
            | cowork_domain::environments::EnvironmentArtifact::LocalLinuxImage { .. }
    ) {
        ensure!(
            report.resource_limits_enforced == Some(true),
            "Actual Linux cgroup resource limit probe failed"
        );
    }
    ensure!(
        report.read_denied
            && report.write_denied
            && report.network_denied
            && report.credentials_isolated
            && report.child_started
            && report.workspace_writable,
        "Isolation boundary probe failed"
    );
    ensure!(
        output.join("qualification-output.txt").is_file(),
        "Qualification output is missing"
    );
    let mut verified = serde_json::json!({"probe":report,"result":result(outcome),"definition":definition,"platform":std::env::consts::OS,"probe_sha256":workspace::digest(&std::fs::read(&config.probe_executable)?)});
    if matches!(
        definition.artifact,
        cowork_domain::environments::EnvironmentArtifact::LinuxKataImage { .. }
    ) {
        verified["boundary"] = serde_json::from_slice(&crate::config::private_read(
            &attempt.join("boundary-report.json"),
            65536,
        )?)?;
        verified["ownership"] = serde_json::from_slice(&crate::config::private_read(
            &attempt.join("ownership.json"),
            65536,
        )?)?;
    }
    if matches!(
        definition.artifact,
        cowork_domain::environments::EnvironmentArtifact::LocalLinuxImage { .. }
    ) {
        verified["local_storage_policy"] = serde_json::json!({"kind":"monitored_soft_budget","bytes":definition.limits.scratch_bytes,"hard_quota":false,"poll_milliseconds":500,"engine_id":config.local_engine_id});
    }
    crate::config::durable_json(&attempt.join("qualification-report.json"), &verified)?;
    // Platform creation verifies containment, networking, and artifact identity.
    // Server/Windows profiles require hard storage enforcement. The explicit local
    // profile reports its separate monitored-storage capability and evidence above.
    Ok((
        cowork_domain::canonical_hash(&verified),
        definition.required_capabilities(),
    ))
}

#[cfg(test)]
mod result_contract_tests {
    use super::*;
    #[test]
    fn actual_supervised_pid_is_retained_as_diagnostic_in_signed_result_contract() {
        let outcome = Outcome {
            termination: cowork_executor::Termination::Exited,
            process_id: Some(37),
            exit_code: Some(0),
            signal: None,
            stdout: Default::default(),
            stderr: Default::default(),
            duration_ms: 1,
            output_complete: true,
        };
        let observed = result(outcome);
        assert_eq!(observed.process_id, Some(37));
        assert!(cowork_domain::processes::validate_process_result(&observed).is_ok());
        let mut missing = observed;
        missing.process_id = None;
        assert!(
            cowork_domain::processes::validate_process_result(&missing).is_err(),
            "A remote adapter must not erase actual PID required by the result contract"
        );
    }
}
