//! gVisor Docker adapter. No host mounts, inherited credentials or host program fallback.
use super::*;
use cowork_domain::{canonical_hash, environments::EnvironmentArtifact};
use serde_json::Value;

fn docker(config: &Configuration) -> Result<&Path> {
    config
        .docker_executable
        .as_deref()
        .context("An absolute Docker executable with runsc runtime is required")
}
fn call(config: &Configuration, args: Vec<String>) -> Result<Outcome> {
    command(docker(config)?, &config.data_directory, args)
}
fn image(definition: &EnvironmentDefinition) -> Result<&str> {
    match &definition.artifact {
        EnvironmentArtifact::LinuxImage { reference }
        | EnvironmentArtifact::LocalLinuxImage { reference } => Ok(reference),
        _ => anyhow::bail!("This receiver installs Linux images only"),
    }
}
fn is_local(definition: &EnvironmentDefinition) -> bool {
    matches!(
        definition.artifact,
        EnvironmentArtifact::LocalLinuxImage { .. }
    )
}
fn runtime(definition: &EnvironmentDefinition) -> &'static str {
    if is_local(definition) {
        "runc"
    } else {
        "runsc"
    }
}
fn local_engine(config: &Configuration, info: &Value) -> Result<()> {
    ensure!(
        config.local_container_profile
            && config
                .local_engine_id
                .as_deref()
                .is_some_and(|id| !id.is_empty()
                    && id.len() <= 256
                    && info.get("ID").and_then(Value::as_str) == Some(id)),
        "Local Docker profile requires an explicit pinned dedicated single-user Engine identity"
    );
    ensure!(
        info.get("OSType").and_then(Value::as_str) == Some("linux"),
        "Local container profile requires a Linux Engine (Docker Desktop VM or dedicated local VM)"
    );
    Ok(())
}
pub fn inspect(config: &Configuration, definition: &EnvironmentDefinition) -> Result<()> {
    if matches!(
        definition.artifact,
        EnvironmentArtifact::LinuxKataImage { .. }
    ) {
        return crate::agent_sandbox::inspect(config, definition);
    }
    let image = image(definition)?;
    let info: Value = serde_json::from_slice(
        &call(
            config,
            vec!["info".into(), "--format".into(), "{{json .}}".into()],
        )?
        .stdout
        .bytes,
    )?;
    if is_local(definition) {
        local_engine(config, &info)?;
    } else {
        ensure!(
            info.pointer("/Runtimes/runsc").is_some(),
            "The runsc gVisor runtime is not installed"
        );
        // Writable-container quotas are verified by create+inspect+qualification; the
        // storage driver must support an explicit per-container size, never a soft monitor.
        ensure!(
            info.get("Driver").and_then(Value::as_str) == Some("overlay2"),
            "Qualified quota adapter requires overlay2/XFS project quotas"
        );
    }
    let observed: Value = serde_json::from_slice(
        &call(config, vec!["image".into(), "inspect".into(), image.into()])?
            .stdout
            .bytes,
    )?;
    if is_local(definition) && image.starts_with("sha256:") {
        ensure!(
            observed
                .get(0)
                .and_then(|v| v.get("Id"))
                .and_then(Value::as_str)
                == Some(image),
            "Exact local image config identity is not installed"
        );
        return Ok(());
    }
    let hashes = observed
        .get(0)
        .and_then(|v| v.get("RepoDigests"))
        .and_then(Value::as_array)
        .context("Image digest not installed")?;
    ensure!(
        hashes.iter().any(|v| v.as_str() == Some(image)),
        "Exact digest image is not locally installed"
    );
    Ok(())
}
fn container(
    config: &Configuration,
    id: &str,
    record: &Ownership,
    definition: Option<&EnvironmentDefinition>,
) -> Result<Value> {
    let observed: Value = serde_json::from_slice(
        &call(config, vec!["inspect".into(), id.into()])?
            .stdout
            .bytes,
    )?;
    let value = observed.get(0).context("Container receipt missing")?;
    ensure!(
        value
            .pointer("/Config/Labels/localai.cowork.nonce")
            .and_then(Value::as_str)
            == Some(&record.nonce)
            && value
                .pointer("/Config/Labels/localai.cowork.binding")
                .and_then(Value::as_str)
                == Some(canonical_hash(&config.binding).as_str()),
        "Foreign container identity"
    );
    if let Some(expected) = &record.physical_id {
        ensure!(
            value.get("Id").and_then(Value::as_str) == Some(expected),
            "Container physical identity changed"
        );
    }
    if let Some(definition) = definition {
        let host = value
            .get("HostConfig")
            .context("Container controls missing")?;
        ensure!(
            host.get("Runtime").and_then(Value::as_str) == Some(runtime(definition))
                && host.get("NetworkMode").and_then(Value::as_str) == Some("none"),
            "Isolation runtime or networking changed"
        );
        ensure!(
            value
                .get("Mounts")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty),
            "Host mounts are forbidden"
        );
        ensure!(
            host.get("Memory").and_then(Value::as_u64) == Some(definition.limits.memory_bytes)
                && host.get("MemorySwap").and_then(Value::as_u64)
                    == Some(definition.limits.memory_bytes)
                && host.get("PidsLimit").and_then(Value::as_u64)
                    == Some(u64::from(definition.limits.processes))
                && host.get("NanoCpus").and_then(Value::as_u64)
                    == Some(u64::from(definition.limits.cpu_millis) * 1_000_000),
            "Hard resource controls do not match definition"
        );
        ensure!(
            host.get("CapDrop")
                .and_then(Value::as_array)
                .is_some_and(|a| a
                    .iter()
                    .any(|v| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("ALL"))))
                && host
                    .get("SecurityOpt")
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("no-new-privileges"))),
            "Container privileges are not restricted"
        );
        if !is_local(definition) {
            ensure!(
                host.pointer("/StorageOpt/size").and_then(Value::as_str)
                    == Some(definition.limits.scratch_bytes.to_string().as_str()),
                "Hard writable-root quota is missing"
            );
        }
        ensure!(
            value.pointer("/Config/Image").and_then(Value::as_str) == Some(image(definition)?),
            "Container image identity changed"
        );
    }
    Ok(value.clone())
}
fn quiesce(config: &Configuration, id: &str, record: &Ownership) -> Result<()> {
    let observed = container(config, id, record, None)?;
    if observed.pointer("/State/Running").and_then(Value::as_bool) == Some(true) {
        call(config, vec!["kill".into(), id.into()])?;
    }
    let observed = container(config, id, record, None)?;
    ensure!(
        observed.pointer("/State/Running").and_then(Value::as_bool) == Some(false)
            && observed.pointer("/State/Pid").and_then(Value::as_u64) == Some(0),
        "Owned container termination is unconfirmed"
    );
    Ok(())
}
/// Explicitly weaker local profile: Docker enforces CPU/memory/PID controls; the
/// writable-layer budget is observed and canceled, never advertised as a quota.
fn supervise_local(
    config: &Configuration,
    definition: &EnvironmentDefinition,
    id: &str,
    launch: &TrustedLaunch,
    cancel: &AtomicBool,
) -> Result<Outcome> {
    use std::sync::atomic::Ordering;
    ensure!(
        writable_bytes(config, id)? <= definition.limits.scratch_bytes,
        "Frozen input already exceeds local monitored storage budget"
    );
    let finished = AtomicBool::new(false);
    let scoped_cancel = AtomicBool::new(cancel.load(Ordering::Acquire));
    let violated = AtomicBool::new(false);
    let monitor_failed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let monitor = scope.spawn(|| {
            let mut last = std::time::Instant::now() - Duration::from_secs(1);
            while !finished.load(Ordering::Acquire) {
                if cancel.load(Ordering::Acquire) {
                    scoped_cancel.store(true, Ordering::Release);
                }
                if last.elapsed() >= Duration::from_millis(500) {
                    let used = writable_bytes(config, id);
                    match used {
                        Ok(bytes) if bytes > definition.limits.scratch_bytes => {
                            violated.store(true, Ordering::Release);
                            scoped_cancel.store(true, Ordering::Release);
                        }
                        Err(_) => {
                            monitor_failed.store(true, Ordering::Release);
                            scoped_cancel.store(true, Ordering::Release);
                        }
                        _ => {}
                    }
                    last = std::time::Instant::now();
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let result = cowork_executor::supervise(launch, &scoped_cancel);
        finished.store(true, Ordering::Release);
        monitor
            .join()
            .map_err(|_| anyhow::anyhow!("Local storage observer failed"))?;
        let result = result?;
        ensure!(
            !monitor_failed.load(Ordering::Acquire),
            "Local writable-layer observation uncertain"
        );
        ensure!(
            !violated.load(Ordering::Acquire),
            "Local writable-layer monitored budget exceeded; physical termination required"
        );
        Ok(result)
    })
}
fn writable_bytes(config: &Configuration, id: &str) -> Result<u64> {
    let out = call(
        config,
        vec![
            "container".into(),
            "inspect".into(),
            "--size".into(),
            "--format".into(),
            "{{.SizeRw}}".into(),
            id.into(),
        ],
    )?;
    Ok(String::from_utf8(out.stdout.bytes)?.trim().parse()?)
}
pub fn recover(config: &Configuration, record: &Ownership) -> Result<()> {
    if record.adapter == "linux_kata" {
        return crate::agent_sandbox::recover(config, record);
    }
    ensure!(
        matches!(
            record.adapter.as_str(),
            "linux_gvisor" | "linux_local_container"
        ) && record.owned_directory.starts_with(&config.data_directory),
        "Foreign adapter journal"
    );
    if record.adapter == "linux_local_container" {
        let info: Value = serde_json::from_slice(
            &call(
                config,
                vec!["info".into(), "--format".into(), "{{json .}}".into()],
            )?
            .stdout
            .bytes,
        )?;
        local_engine(config, &info)?;
    }
    if let Some(id) = &record.physical_id {
        ensure!(
            id.len() == 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "Invalid owned container ID"
        );
        let observed = call(
            config,
            vec![
                "container".into(),
                "ls".into(),
                "--all".into(),
                "--no-trunc".into(),
                "--filter".into(),
                format!("id={id}"),
                "--format".into(),
                "{{.ID}}".into(),
            ],
        )?;
        let found = String::from_utf8(observed.stdout.bytes)?.trim().to_owned();
        if found.is_empty() {
            return Ok(());
        }
        ensure!(found == *id, "Owned container lookup was ambiguous");
        quiesce(config, id, record)?;
        return Ok(());
    }
    // Name and two private ownership labels permit termination only, never adoption
    // of a launch permit after a crash between create and its physical-ID receipt.
    let name = format!("cowork-v2-{}", record.nonce);
    let observed = call(
        config,
        vec![
            "container".into(),
            "ls".into(),
            "--all".into(),
            "--no-trunc".into(),
            "--filter".into(),
            format!("name=^/{name}$"),
            "--format".into(),
            "{{.ID}}".into(),
        ],
    )?;
    let id = String::from_utf8(observed.stdout.bytes)?.trim().to_owned();
    if id.is_empty() {
        return Ok(());
    }
    quiesce(config, &id, record)
}
pub fn release(config: &Configuration, record: &Ownership) -> Result<()> {
    if record.adapter == "linux_kata" {
        return crate::agent_sandbox::release(config, record);
    }
    recover(config, record)?;
    let filter = record
        .physical_id
        .as_ref()
        .map(|id| format!("id={id}"))
        .unwrap_or_else(|| format!("name=^/cowork-v2-{}$", record.nonce));
    let observed = call(
        config,
        vec![
            "container".into(),
            "ls".into(),
            "--all".into(),
            "--no-trunc".into(),
            "--filter".into(),
            filter,
            "--format".into(),
            "{{.ID}}".into(),
        ],
    )?;
    let id = String::from_utf8(observed.stdout.bytes)?.trim().to_owned();
    if id.is_empty() {
        return Ok(());
    }
    quiesce(config, &id, record)?;
    // Never --force: Docker must agree the exact labelled physical container is stopped.
    call(config, vec!["container".into(), "rm".into(), id])?;
    Ok(())
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
    if matches!(
        definition.artifact,
        EnvironmentArtifact::LinuxKataImage { .. }
    ) {
        return crate::agent_sandbox::run(
            config,
            definition,
            invocation,
            crate::agent_sandbox::RunPaths { input, attempt },
            cancel,
            probe,
            None,
        );
    }
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cowork-v2-{nonce}");
    let mut record = Ownership {
        nonce,
        adapter: if is_local(definition) {
            "linux_local_container".into()
        } else {
            "linux_gvisor".into()
        },
        physical_id: None,
        mount_volume: None,
        artifact: definition.artifact.sha256().into(),
        definition_sha256: canonical_hash(definition),
        owned_directory: attempt.to_path_buf(),
        agent_sandbox: None,
    };
    let owner_path = attempt.join("ownership.json");
    crate::config::durable_json(&owner_path, &record)?;
    let program = if probe {
        "/cowork-runner-probe".into()
    } else {
        format!(
            "/{}",
            definition
                .programs
                .get(&invocation.program)
                .context("Program alias is not installed")?
        )
    };
    let mut args = vec![
        "create".into(),
        "--pull=never".into(),
        "--name".into(),
        name.clone(),
        format!("--runtime={}", runtime(definition)),
        "--network=none".into(),
        "--cap-drop=ALL".into(),
        "--security-opt=no-new-privileges".into(),
        "--memory".into(),
        definition.limits.memory_bytes.to_string(),
        "--memory-swap".into(),
        definition.limits.memory_bytes.to_string(),
        "--pids-limit".into(),
        definition.limits.processes.to_string(),
        "--cpus".into(),
        format!(
            "{}.{:03}",
            definition.limits.cpu_millis / 1000,
            definition.limits.cpu_millis % 1000
        ),
        "--label".into(),
        format!("localai.cowork.nonce={}", record.nonce),
        "--label".into(),
        format!("localai.cowork.binding={}", canonical_hash(&config.binding)),
        "--workdir=/workspace".into(),
        "--entrypoint".into(),
        program,
        image(definition)?.into(),
    ];
    if !is_local(definition) {
        args.splice(
            1..1,
            [
                "--storage-opt".into(),
                format!("size={}", definition.limits.scratch_bytes),
            ],
        );
    }
    args.extend(invocation.arguments.clone());
    let created = call(config, args)?;
    let id = String::from_utf8(created.stdout.bytes)?.trim().to_owned();
    ensure!(
        id.len() == 64 && id.bytes().all(|v| v.is_ascii_hexdigit()),
        "Invalid Docker create receipt"
    );
    record.physical_id = Some(id.clone());
    crate::config::durable_json(&owner_path, &record)?;
    let execute = (|| -> Result<Outcome> {
        container(config, &id, &record, Some(definition))?;
        call(
            config,
            vec![
                "cp".into(),
                format!("{}/.", input.display()),
                format!("{id}:/workspace"),
            ],
        )?;
        if probe {
            call(
                config,
                vec![
                    "cp".into(),
                    config.probe_executable.to_string_lossy().into_owned(),
                    format!("{id}:/cowork-runner-probe"),
                ],
            )?;
        }
        let launch = trusted(
            docker(config)?,
            &config.data_directory,
            vec!["start".into(), "--attach".into(), id.clone()],
            Duration::from_secs(invocation.timeout_seconds),
        )?;
        let mut outcome = if is_local(definition) {
            supervise_local(config, definition, &id, &launch, cancel)?
        } else {
            cowork_executor::supervise(&launch, cancel)?
        };
        quiesce(config, &id, &record)?;
        let observed = container(config, &id, &record, Some(definition))?;
        if outcome.termination == cowork_executor::Termination::Exited {
            outcome.exit_code = observed.pointer("/State/ExitCode").and_then(Value::as_i64);
        }
        // Diagnostic PID of the actually supervised Docker CLI in this receiver's
        // namespace. Container ownership and stop authority remain the exact
        // engine-observed container ID, labels and positive stopped receipt.
        Ok(outcome)
    })();
    // This cleanup is independent of the HTTP caller and applies on every adapter
    // error as well. A failed physical observation never produces a success receipt.
    let stopped = quiesce(config, &id, &record);
    let outcome = execute?;
    stopped?;
    let output = attempt.join("output");
    crate::config::private_directory(&output)?;
    call(
        config,
        vec![
            "cp".into(),
            format!("{id}:/workspace/."),
            output.to_string_lossy().into_owned(),
        ],
    )?;
    workspace::files(&output)?;
    Ok((outcome, output))
}
