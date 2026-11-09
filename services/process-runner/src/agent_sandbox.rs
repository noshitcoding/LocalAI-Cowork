//! In-cluster Kata adapter. Kubernetes credentials stay in this trusted receiver.
//! Every untrusted Start is one durable attempt, never retried after uncertainty.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
use crate::{
    config::{self, Configuration},
    platform::Ownership,
    workspace,
};
use anyhow::{Context, Result, ensure};
use cowork_agent_sandbox::runtime::{Execution, Runtime};
use cowork_agent_sandbox::{ControlPlane, OwnedSandbox, Resources};
use cowork_contracts::ProcessInvocation;
use cowork_domain::{
    canonical_hash,
    environments::{EnvironmentArtifact, EnvironmentDefinition, EnvironmentNetwork},
};
use cowork_executor::Outcome;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunOwnership {
    pub control_sha256: String,
    pub primary: OwnedSandbox,
    pub exporter: Option<OwnedSandbox>,
    pub pvc_name: String,
    pub pvc_uid: Option<String>,
    pub policy_uid: Option<String>,
    pub owns_pvc: bool,
}
pub fn block_on<T: Send>(future: impl Future<Output = Result<T>> + Send) -> Result<T> {
    // inspect/recovery can be called from a Tokio request or a blocking supervisor.
    // A separate owned thread avoids nested runtime entry and keeps cleanup alive.
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?
                    .block_on(future)
            })
            .join()
            .map_err(|_| anyhow::anyhow!("Sandbox supervisor thread failed"))?
    })
}
pub fn client(config: &Configuration) -> Result<ControlPlane> {
    ensure!(
        cfg!(target_os = "linux"),
        "Agent Sandbox receiver must run on Linux"
    );
    let c = config
        .agent_sandbox
        .clone()
        .context("Agent Sandbox operator configuration required")?;
    ensure!(
        c.worker_label_value == canonical_hash(&config.binding)[..32],
        "Worker binding label mismatch"
    );
    ControlPlane::new(c)
}
pub fn image(definition: &EnvironmentDefinition) -> Result<&str> {
    match &definition.artifact {
        EnvironmentArtifact::LinuxKataImage { reference } => Ok(reference),
        _ => anyhow::bail!("Kata artifact required"),
    }
}
/// Admission policy can revoke future launches without changing the authority
/// required to terminate an already owned physical Pod. Cluster/namespace/TLS,
/// runtime, storage and runner identity remain part of this fingerprint.
pub fn control_hash(config: &cowork_agent_sandbox::Configuration) -> String {
    let mut ownership = config.clone();
    ownership.approved_images.clear();
    canonical_hash(&ownership)
}
pub fn proxy<'a>(
    plane: &'a ControlPlane,
    definition: &EnvironmentDefinition,
) -> Result<Option<&'a cowork_agent_sandbox::control::WebProxyPolicy>> {
    match &definition.network {
        None | Some(EnvironmentNetwork::Denied) => Ok(None),
        Some(EnvironmentNetwork::WebProxy { policy_id }) => Ok(Some(
            plane
                .config()
                .web_proxy_policies
                .get(policy_id)
                .context("Requested proxy policy is not configured by operator")?,
        )),
    }
}
pub fn resources(definition: &EnvironmentDefinition) -> Resources {
    Resources {
        memory_bytes: definition.limits.memory_bytes,
        cpu_millis: definition.limits.cpu_millis,
        scratch_bytes: definition.limits.scratch_bytes,
    }
}
pub fn token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
pub fn inspect(config: &Configuration, definition: &EnvironmentDefinition) -> Result<()> {
    definition.validate()?;
    image(definition)?;
    let plane = client(config)?;
    plane.config().approve_image(image(definition)?)?;
    proxy(&plane, definition)?;
    block_on(plane.inspect())
}
fn save(record: &Ownership) -> Result<()> {
    config::durable_json(&record.owned_directory.join("ownership.json"), record)
}
fn verify_journal(config: &Configuration, record: &Ownership) -> Result<()> {
    ensure!(
        record
            .agent_sandbox
            .as_ref()
            .context("Kata ownership missing")?
            .control_sha256
            == control_hash(
                config
                    .agent_sandbox
                    .as_ref()
                    .context("Kubernetes configuration missing")?
            ),
        "Kubernetes operator configuration changed since physical ownership"
    );
    ensure!(
        record.adapter == "linux_kata"
            && record.owned_directory.starts_with(&config.data_directory)
            && record.nonce.len() == 32
            && record.nonce.bytes().all(|b| b.is_ascii_hexdigit())
            && record.agent_sandbox.is_some(),
        "Foreign Kata ownership journal"
    );
    Ok(())
}
async fn stop_one(plane: &ControlPlane, record: &mut Ownership, exporter: bool) -> Result<()> {
    let state = record
        .agent_sandbox
        .as_ref()
        .context("Kata journal missing")?;
    let original = if exporter {
        state.exporter.clone()
    } else {
        Some(state.primary.clone())
    };
    let Some(mut owned) = original else {
        return Ok(());
    };
    if owned.uid.is_none() {
        if let Some(value) = plane.get("sandboxes", &owned.name).await? {
            plane.verify_owned(&value, &owned)?;
            owned.uid = Some(cowork_agent_sandbox::control::field(&value, "/metadata/uid")?.into());
            let state = record.agent_sandbox.as_mut().unwrap();
            if exporter {
                state.exporter = Some(owned.clone())
            } else {
                state.primary = owned.clone()
            }
            save(record)?;
        } else {
            return Ok(());
        }
    }
    plane
        .suspend(&owned, |receipt| {
            let state = record.agent_sandbox.as_mut().unwrap();
            let target = if exporter {
                state.exporter.as_mut().unwrap()
            } else {
                &mut state.primary
            };
            target.stopped = Some(receipt.clone());
            save(record)
        })
        .await?;
    Ok(())
}
pub fn recover(config: &Configuration, record: &Ownership) -> Result<()> {
    verify_journal(config, record)?;
    let plane = client(config)?;
    let mut record = record.clone();
    block_on(async {
        plane.inspect().await?;
        stop_one(&plane, &mut record, false).await?;
        stop_one(&plane, &mut record, true).await
    })
}
pub fn release(config: &Configuration, record: &Ownership) -> Result<()> {
    verify_journal(config, record)?;
    let plane = client(config)?;
    let mut record = record.clone();
    block_on(async {
        plane.inspect().await?;
        stop_one(&plane, &mut record, false).await?;
        stop_one(&plane, &mut record, true).await?;
        let state = record.agent_sandbox.as_ref().unwrap();
        if let Some(exporter) = &state.exporter {
            plane.delete_sandbox(exporter).await?;
        }
        plane.delete_sandbox(&state.primary).await?;
        // Wait until API confirms no Pod before deleting an app-owned PVC. A shared
        // lifecycle workspace is never removed by attempt retention.
        plane
            .delete_app_resource(
                "networkpolicies",
                &state.primary.name,
                &record.nonce,
                state.policy_uid.as_deref(),
            )
            .await?;
        if state.owns_pvc {
            plane
                .delete_app_resource(
                    "persistentvolumeclaims",
                    &state.pvc_name,
                    &record.nonce,
                    state.pvc_uid.as_deref(),
                )
                .await?;
        }
        Ok(())
    })
}
fn checked_execution(result: &Execution) -> Result<()> {
    ensure!(
        result.complete
            && result.exit_code == Some(0)
            && !result.stdout.truncated
            && !result.stderr.truncated,
        "Trusted sandbox probe failed"
    );
    Ok(())
}
pub async fn boundary(
    config: &Configuration,
    definition: &EnvironmentDefinition,
    runtime: &mut Runtime,
    plane: &ControlPlane,
    attempt: &Path,
) -> Result<()> {
    let args = vec![
        "/usr/local/bin/cowork-runner-probe".into(),
        "--boundary".into(),
        definition.limits.memory_bytes.to_string(),
        definition.limits.cpu_millis.to_string(),
        definition.limits.processes.to_string(),
        definition.limits.scratch_bytes.to_string(),
    ];
    let measured = runtime
        .execute(
            args,
            HashMap::new(),
            Duration::from_secs(20),
            &AtomicBool::new(false),
            65536,
        )
        .await?;
    checked_execution(&measured)?;
    let mut report: serde_json::Value = serde_json::from_slice(&measured.stdout.bytes)?;
    ensure!(
        report["limits_enforced"] == true
            && report["executable_sha256"].as_str()
                == Some(workspace::digest(&std::fs::read(&config.probe_executable)?).as_str()),
        "Trusted boundary executable or enforcement changed"
    );
    let proxy = proxy(plane, definition)?;
    let canary = proxy
        .map(|p| p.direct_probe_address.as_str())
        .or(plane.config().egress_probe_address.as_deref())
        .context("Operator must configure a reachable non-loopback egress canary")?;
    let address: std::net::SocketAddr = canary.parse()?;
    tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(address),
    )
    .await
    .context("Egress canary unobservable from trusted runner")??;
    if let Some(proxy) = proxy {
        let measured = runtime
            .execute(
                vec![
                    "/usr/local/bin/cowork-runner-probe".into(),
                    "--proxy-probe".into(),
                    proxy.url.clone(),
                    proxy.probe_url.clone(),
                    proxy.direct_probe_address.clone(),
                ],
                HashMap::new(),
                Duration::from_secs(20),
                &AtomicBool::new(false),
                65536,
            )
            .await?;
        checked_execution(&measured)?;
        let network: serde_json::Value = serde_json::from_slice(&measured.stdout.bytes)?;
        ensure!(
            network["direct_denied"] == true && network["proxy_reachable"] == true,
            "Proxy policy behavioral probe failed"
        );
        report["network"] = network;
    } else {
        let measured = runtime
            .execute(
                vec![
                    "/usr/local/bin/cowork-runner-probe".into(),
                    "--egress-denied".into(),
                    canary.into(),
                ],
                HashMap::new(),
                Duration::from_secs(10),
                &AtomicBool::new(false),
                65536,
            )
            .await?;
        checked_execution(&measured)?;
        let network: serde_json::Value = serde_json::from_slice(&measured.stdout.bytes)?;
        ensure!(
            network["direct_denied"] == true,
            "Direct egress was reachable"
        );
        report["network"] = network;
    }
    config::durable_json(&attempt.join("boundary-report.json"), &report)?;
    Ok(())
}
pub async fn stage(
    runtime: &mut Runtime,
    input: &Path,
    manifest: &workspace::Manifest,
    cancel: &AtomicBool,
) -> Result<()> {
    checked_execution(
        &runtime
            .execute(
                vec![
                    "/usr/local/bin/cowork-runner-probe".into(),
                    "--reset".into(),
                    "/workspace".into(),
                ],
                HashMap::new(),
                Duration::from_secs(20),
                cancel,
                65536,
            )
            .await?,
    )?;
    let mut directories = Vec::new();
    for (path, entry) in manifest {
        if entry.directory {
            directories.push(path.clone());
            continue;
        }
        let bytes = std::fs::read(input.join(path))?;
        ensure!(
            bytes.len() as u64 == entry.bytes && workspace::digest(&bytes) == entry.sha256,
            "Input changed after frozen snapshot"
        );
        runtime.put(path, bytes, "0644").await?;
    }
    for chunk in directories.chunks(500) {
        let mut argv = vec![
            "/usr/local/bin/cowork-runner-probe".into(),
            "--directories".into(),
        ];
        argv.extend_from_slice(chunk);
        checked_execution(
            &runtime
                .execute(argv, HashMap::new(), Duration::from_secs(20), cancel, 65536)
                .await?,
        )?;
    }
    Ok(())
}
pub struct RunPaths<'a> {
    pub input: &'a Path,
    pub attempt: &'a Path,
}

pub fn run(
    config: &Configuration,
    definition: &EnvironmentDefinition,
    invocation: &ProcessInvocation,
    paths: RunPaths<'_>,
    cancel: &AtomicBool,
    probe: bool,
    workspace_id: Option<&str>,
) -> Result<(Outcome, PathBuf)> {
    let RunPaths { input, attempt } = paths;
    let plane = client(config)?;
    let image = image(definition)?.to_owned();
    plane.config().approve_image(&image)?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cw-{nonce}");
    let shared = if let Some(workspace_id) = workspace_id {
        Some(crate::sandbox_lifecycle::checkout(
            config,
            workspace_id,
            definition,
        )?)
    } else {
        ensure!(
            probe,
            "Create an explicit workspace sandbox session before executing Kata tools"
        );
        None
    };
    let (pvc_name, pvc_uid, owns_pvc) = if let Some((name, uid)) = shared {
        (name, Some(uid), false)
    } else {
        (format!("cw-data-{nonce}"), None, true)
    };
    let primary = OwnedSandbox {
        name: name.clone(),
        nonce: nonce.clone(),
        uid: None,
        pod: None,
        stopped: None,
    };
    let mut record = Ownership {
        nonce: nonce.clone(),
        adapter: "linux_kata".into(),
        physical_id: None,
        mount_volume: Some(pvc_name.clone()),
        artifact: definition.artifact.sha256().into(),
        definition_sha256: canonical_hash(definition),
        owned_directory: attempt.into(),
        agent_sandbox: Some(RunOwnership {
            control_sha256: control_hash(plane.config()),
            primary,
            exporter: None,
            pvc_name: pvc_name.clone(),
            pvc_uid,
            policy_uid: None,
            owns_pvc,
        }),
    };
    save(&record)?;
    let auth_token = token();
    config::write_new(&attempt.join("sandboxd-token"), auth_token.as_bytes())?;
    let auth_digest = workspace::digest(auth_token.as_bytes());
    let execute = block_on(async {
        plane.inspect().await?;
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Attempt canceled before Kubernetes mutation"
        );
        if owns_pvc {
            let uid = plane
                .create_pvc(&pvc_name, &nonce, definition.limits.scratch_bytes)
                .await?;
            record.agent_sandbox.as_mut().unwrap().pvc_uid = Some(uid);
            save(&record)?;
        }
        let policy_uid = plane
            .create_policy(&name, &nonce, proxy(&plane, definition)?)
            .await?;
        record.agent_sandbox.as_mut().unwrap().policy_uid = Some(policy_uid);
        save(&record)?;
        plane
            .verify_policy(
                &name,
                &nonce,
                record
                    .agent_sandbox
                    .as_ref()
                    .unwrap()
                    .policy_uid
                    .as_deref()
                    .unwrap(),
                proxy(&plane, definition)?,
            )
            .await?;
        let uid = plane
            .create(
                &record.agent_sandbox.as_ref().unwrap().primary,
                &pvc_name,
                &image,
                &resources(definition),
                &auth_digest,
                false,
            )
            .await?;
        record.physical_id = Some(uid.clone());
        record.agent_sandbox.as_mut().unwrap().primary.uid = Some(uid);
        save(&record)?;
        let (pod, host) = plane
            .wait_ready(&record.agent_sandbox.as_ref().unwrap().primary, &image)
            .await?;
        record.agent_sandbox.as_mut().unwrap().primary.pod = Some(pod);
        save(&record)?;
        plane
            .fence_pod(&record.agent_sandbox.as_ref().unwrap().primary)
            .await?;
        let mut runtime = Runtime::connect(&host, &auth_token).await?;
        boundary(config, definition, &mut runtime, &plane, attempt).await?;
        let manifest = workspace::files(input)?;
        stage(&mut runtime, input, &manifest, cancel).await?;
        // Consumed one-shot launch permit is additionally journaled immediately before
        // the sole untrusted RPC. Recovery is termination-only even if Init was lost.
        config::write_new(&attempt.join("sandboxd-dispatch-committed"), b"no replay")?;
        config::sync_directory(attempt)?;
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Attempt canceled before Start"
        );
        let program = if probe {
            "/usr/local/bin/cowork-runner-probe".into()
        } else {
            format!(
                "/{}",
                definition
                    .programs
                    .get(&invocation.program)
                    .context("Unknown environment alias")?
            )
        };
        let mut command = vec![program];
        command.extend(invocation.arguments.clone());
        let env = proxy(&plane, definition)?
            .map(|p| p.environment())
            .unwrap_or_default();
        let mut environment: BTreeMap<String, String> = env.into_iter().collect();
        environment.insert("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into());
        environment.insert("HOME".into(), "/workspace".into());
        environment.insert("TMPDIR".into(), "/tmp".into());
        let supervised = serde_json::json!({"program":command[0],"arguments":&command[1..],"environment":environment,"timeout_seconds":invocation.timeout_seconds});
        let result = runtime
            .execute(
                vec![
                    "/usr/local/bin/cowork-runner-probe".into(),
                    "--supervise".into(),
                    serde_json::to_string(&supervised)?,
                ],
                HashMap::new(),
                Duration::from_secs(invocation.timeout_seconds.saturating_add(10)),
                cancel,
                1024 * 1024,
            )
            .await;
        drop(runtime);
        stop_one(&plane, &mut record, false).await?;
        let result = result?;
        checked_execution(&result)?;
        let supervised_outcome: Outcome = serde_json::from_slice(&result.stdout.bytes)
            .context("Trusted command supervisor outcome missing; never replay")?;
        // Preserve the actual guest child PID for diagnostics. The exact Pod UID
        // and positive physical stop receipt establish containment and authority.
        let exporter = OwnedSandbox {
            name: format!("cwe-{nonce}"),
            nonce: nonce.clone(),
            uid: None,
            pod: None,
            stopped: None,
        };
        record.agent_sandbox.as_mut().unwrap().exporter = Some(exporter);
        save(&record)?;
        let uid = plane
            .create(
                record
                    .agent_sandbox
                    .as_ref()
                    .unwrap()
                    .exporter
                    .as_ref()
                    .unwrap(),
                &pvc_name,
                &image,
                &resources(definition),
                &auth_digest,
                true,
            )
            .await?;
        record
            .agent_sandbox
            .as_mut()
            .unwrap()
            .exporter
            .as_mut()
            .unwrap()
            .uid = Some(uid);
        save(&record)?;
        let (pod, host) = plane
            .wait_ready(
                record
                    .agent_sandbox
                    .as_ref()
                    .unwrap()
                    .exporter
                    .as_ref()
                    .unwrap(),
                &image,
            )
            .await?;
        record
            .agent_sandbox
            .as_mut()
            .unwrap()
            .exporter
            .as_mut()
            .unwrap()
            .pod = Some(pod);
        save(&record)?;
        plane
            .fence_pod(
                record
                    .agent_sandbox
                    .as_ref()
                    .unwrap()
                    .exporter
                    .as_ref()
                    .unwrap(),
            )
            .await?;
        let mut runtime = Runtime::connect(&host, &auth_token).await?;
        let scan = runtime
            .execute(
                vec![
                    "/usr/local/bin/cowork-runner-probe".into(),
                    "--manifest".into(),
                    "/workspace".into(),
                ],
                HashMap::new(),
                Duration::from_secs(60),
                &AtomicBool::new(false),
                4 * 1024 * 1024,
            )
            .await?;
        checked_execution(&scan)?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Manifest {
            executable_sha256: String,
            entries: BTreeMap<String, workspace::Entry>,
        }
        let scan: Manifest = serde_json::from_slice(&scan.stdout.bytes)?;
        ensure!(
            scan.executable_sha256 == workspace::digest(&std::fs::read(&config.probe_executable)?),
            "Immutable export probe changed"
        );
        let output = attempt.join("output");
        config::private_directory(&output)?;
        let mut total = 0u64;
        ensure!(scan.entries.len() <= 10000, "Too many output paths");
        for (path, entry) in &scan.entries {
            workspace::relative(path)?;
            if entry.directory {
                std::fs::create_dir_all(output.join(path))?;
                continue;
            }
            total = total
                .checked_add(entry.bytes)
                .context("Output size overflow")?;
            ensure!(
                entry.bytes <= 64 * 1024 * 1024 && total <= 512 * 1024 * 1024,
                "Output bound exceeded"
            );
            let bytes = runtime.get(path, entry.bytes as usize).await?;
            ensure!(
                bytes.len() as u64 == entry.bytes && workspace::digest(&bytes) == entry.sha256,
                "Output changed during read-only export"
            );
            let dest = output.join(path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            config::write_new(&dest, &bytes)?;
        }
        ensure!(
            workspace::files(&output)? == scan.entries,
            "Output manifest differs"
        );
        drop(runtime);
        stop_one(&plane, &mut record, true).await?;
        Ok((supervised_outcome, output))
    });
    // Physical ownership persists independently of the request future on every error.
    let stop = block_on(async {
        stop_one(&plane, &mut record, false).await?;
        stop_one(&plane, &mut record, true).await
    });
    stop?;
    execute
}
#[cfg(test)]
mod ownership_tests {
    use super::*;
    #[test]
    fn image_revocation_preserves_cleanup_authority_but_namespace_change_does_not() {
        let mut config: cowork_agent_sandbox::Configuration = serde_json::from_value(serde_json::json!({
            "api_url":"https://kubernetes.default.svc/", "namespace":"tenant", "namespace_uid":"original-namespace",
            "token_file":"/run/token", "ca_file":"/run/ca", "runtime_class":"kata", "runtime_handler":"kata",
            "storage_class":"qualified", "pvc_access_mode":"ReadWriteOncePod", "worker_namespace":"control", "worker_label_value":"a".repeat(32),
            "approved_images":[format!("registry.example/runtime@sha256:{}","b".repeat(64))]
        })).unwrap();
        let identity = control_hash(&config);
        let revoked = config.approved_images[0].clone();
        config.approved_images = vec![format!(
            "registry.example/runtime@sha256:{}",
            "c".repeat(64)
        )];
        assert_eq!(control_hash(&config), identity);
        assert!(config.approve_image(&revoked).is_err());
        config.namespace_uid = "replacement-namespace".into();
        assert_ne!(control_hash(&config), identity);
    }
}
