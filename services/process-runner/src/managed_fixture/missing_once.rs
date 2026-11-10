//! One actual native Missing observation. Raw private diagnostics are evidence,
//! never authority or a reason to retry. No chunks, start, fence or Controller.
use super::*;
use anyhow::{Result, ensure};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions as CapOptions, OpenOptionsExt as CapOptionsExt};
use std::{
    fs::{Metadata, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
};

const BYTES: usize = 16 * 1024;
const BEFORE: &str = "f51-missing-once-slot-before.bin";
const RAW: &str = "f51-missing-once-slot.bin";
const REPORT: &str = "f51-missing-once-observation.json";

fn identity(m: &Metadata) -> serde_json::Value {
    serde_json::json!({"dev":m.dev(),"ino":m.ino(),"uid":m.uid(),"mode":m.mode(),
        "nlink":m.nlink(),"bytes":m.len(),"blocks":m.blocks()})
}
fn save(base: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(base.join(name))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    let mut copy = vec![0; bytes.len()];
    file.read_exact_at(&mut copy, 0)?;
    ensure!(
        copy == bytes && file.metadata()?.len() == bytes.len() as u64,
        "Own evidence readback differs"
    );
    File::open(base)?.sync_all()?;
    Ok(())
}

struct Slot {
    directory: Dir,
    file: File,
    original: Metadata,
    before_sha256: String,
}
impl Slot {
    fn open(state: &Path, base: &Path) -> Result<Self> {
        let parent = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(state)?;
        let m = parent.metadata()?;
        ensure!(
            m.is_dir() && m.uid() == 0 && m.mode() & 0o777 == 0o700,
            "Own state directory differs"
        );
        let directory = Dir::from_std_file(parent);
        let mut options = CapOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
        let file = directory
            .open_with("managed-failure.bin", &options)?
            .into_std();
        let original = file.metadata()?;
        ensure!(
            original.is_file()
                && original.uid() == 0
                && original.nlink() == 1
                && original.mode() & 0o777 == 0o600
                && original.len() == BYTES as u64
                && original
                    .blocks()
                    .checked_mul(512)
                    .is_some_and(|n| n >= BYTES as u64),
            "Original diagnostic slot metadata differs"
        );
        let mut bytes = vec![0; BYTES];
        file.read_exact_at(&mut bytes, 0)?;
        save(base, BEFORE, &bytes)?;
        Ok(Self {
            directory,
            file,
            original,
            before_sha256: hash(&bytes),
        })
    }

    fn capture(
        &self,
        base: &Path,
        input_hash: Option<&str>,
        attempt_hash: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mut options = CapOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
        let current = self
            .directory
            .open_with("managed-failure.bin", &options)?
            .into_std()
            .metadata()?;
        let before = self.file.metadata()?;
        ensure!(
            before.is_file()
                && before.uid() == 0
                && before.nlink() == 1
                && before.mode() & 0o777 == 0o600
                && before.len() == BYTES as u64
                && before
                    .blocks()
                    .checked_mul(512)
                    .is_some_and(|n| n >= BYTES as u64)
                && before.dev() == self.original.dev()
                && before.ino() == self.original.ino()
                && current.dev() == before.dev()
                && current.ino() == before.ino(),
            "Original diagnostic slot identity changed"
        );
        let mut bytes = vec![0; BYTES];
        self.file.read_exact_at(&mut bytes, 0)?;
        // Preserve exact original bytes before parsing, HTTP body consumption,
        // any further managed request, service shutdown or error cleanup.
        save(base, RAW, &bytes)?;
        let after = self.file.metadata()?;
        let rebound = self
            .directory
            .open_with("managed-failure.bin", &options)?
            .into_std()
            .metadata()?;
        ensure!(
            after.dev() == before.dev()
                && after.ino() == before.ino()
                && after.len() == before.len()
                && after.mtime() == before.mtime()
                && after.mtime_nsec() == before.mtime_nsec()
                && rebound.dev() == after.dev()
                && rebound.ino() == after.ino(),
            "Diagnostic changed during original read"
        );
        let digest = hash(&bytes);
        let length = u32::from_le_bytes(bytes[..4].try_into()?) as usize;
        let mut summary = serde_json::json!({"file":RAW,"bytes":BYTES,"sha256":digest,
            "before_sha256":self.before_sha256,"original_file":identity(&before),
            "raw_changed":digest!=self.before_sha256,"declared_payload_bytes":length,
            "frame_valid":false,"payload_sha256":null,"correlated":false,"phase":null,"transfer_stage_candidates":[],
            "error_payload_exported":false,"authority":false});
        if length == 0 {
            let empty = bytes.iter().all(|v| *v == 0);
            summary["frame_valid"] = empty.into();
            summary["empty"] = empty.into();
            return Ok(summary);
        }
        if length > BYTES - 36 {
            return Ok(summary);
        }
        let payload = &bytes[36..36 + length];
        if ring::digest::digest(&ring::digest::SHA256, payload).as_ref() != &bytes[4..36]
            || bytes[36 + length..].iter().any(|v| *v != 0)
        {
            return Ok(summary);
        }
        summary["payload_sha256"] = hash(payload).into();
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) else {
            return Ok(summary);
        };
        let Some(object) = value.as_object() else {
            return Ok(summary);
        };
        let keys = [
            "format_version",
            "diagnostic_only",
            "phase",
            "attempt_sha256",
            "error",
            "clock_rollback",
        ];
        if object.len() != keys.len()
            || keys.iter().any(|key| !object.contains_key(*key))
            || value["format_version"] != 1
            || value["diagnostic_only"] != true
            || !value["phase"].as_str().is_some_and(|s| s.len() <= 128)
            || !value["attempt_sha256"].as_str().is_some_and(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
            || !value["error"]
                .as_str()
                .is_some_and(|s| s.chars().count() <= 1024)
        {
            return Ok(summary);
        }
        summary["frame_valid"] = true.into();
        let expected = match value["phase"].as_str() {
            Some("managed_input_missing_absent") => input_hash,
            Some("managed_input_missing" | "managed_input_missing_response") => attempt_hash,
            _ => None,
        };
        let correlated = digest != self.before_sha256
            && expected.is_some_and(|h| value["attempt_sha256"].as_str() == Some(h));
        summary["correlated"] = correlated.into();
        if correlated {
            summary["phase"] = value["phase"].clone();
            let error = value["error"].as_str().unwrap();
            let stages = [
                "missing_receiver_current",
                "missing_authorize_input",
                "missing_current",
                "current_custody",
                "missing_snapshot",
            ]
            .into_iter()
            .filter(|stage| error.contains(stage))
            .collect::<Vec<_>>();
            summary["transfer_stage_candidates"] = serde_json::to_value(stages)?;
            summary["stage_candidates_are_bounded_Debug_projection"] = true.into();
        }
        Ok(summary)
    }
}

fn start_ticks(pid: u32) -> Result<Option<u64>> {
    let text = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let tail = text
        .rsplit_once(')')
        .ok_or_else(|| anyhow::anyhow!("Invalid own process stat"))?
        .1;
    Ok(Some(
        tail.split_whitespace()
            .nth(19)
            .ok_or_else(|| anyhow::anyhow!("Missing own start identity"))?
            .parse()?,
    ))
}
fn process_identity(service: &mut OwnService) -> Result<serde_json::Value> {
    ensure!(
        service.0.try_wait()?.is_none(),
        "Original service is not live"
    );
    let pid = service.0.id();
    let ticks = start_ticks(pid)?.ok_or_else(|| anyhow::anyhow!("Original service stat absent"))?;
    let file = File::open(format!("/proc/{pid}/exe"))?;
    let m = file.metadata()?;
    ensure!(
        m.is_file() && m.len() <= 128 * 1024 * 1024,
        "Own service executable bound"
    );
    let mut bytes = Vec::new();
    file.take(128 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    let digest = hash(&bytes);
    ensure!(
        digest == std::env::var("COWORK_TEST_SERVICE_SHA256")?,
        "Original service executable differs"
    );
    Ok(
        serde_json::json!({"pid":pid,"start_ticks":ticks,"executable_dev":m.dev(),"executable_ino":m.ino(),"executable_sha256":digest}),
    )
}
fn task_records(case: &Path) -> Result<usize> {
    let volumes = case.join("volumes");
    let entries = match std::fs::read_dir(&volumes) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let mut count = 0;
    for (index, volume) in entries.enumerate() {
        ensure!(index < 128, "Own volume observation bound");
        let volume = volume?;
        ensure!(volume.file_type()?.is_dir(), "Own volume directory differs");
        let root = volume.path().join("root");
        match root.symlink_metadata() {
            Ok(m) => ensure!(m.is_dir() && m.uid() == 0, "Own volume root differs"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        }
        let children = match std::fs::read_dir(root) {
            Ok(children) => children,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        for (index, child) in children.enumerate() {
            ensure!(index < 128, "Own namespace observation bound");
            let child = child?;
            if !child.file_name().to_string_lossy().starts_with("cw-") {
                continue;
            }
            ensure!(child.file_type()?.is_dir(), "Own control directory differs");
            for name in ["task.a", "task.b"] {
                let file = match OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
                    .open(child.path().join(name))
                {
                    Ok(file) => file,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e.into()),
                };
                let m = file.metadata()?;
                ensure!(
                    m.is_file() && m.uid() == 0 && m.nlink() == 1 && m.len() == BYTES as u64,
                    "Own task slot differs"
                );
                let mut bytes = vec![0; BYTES];
                file.read_exact_at(&mut bytes, 0)?;
                if bytes.iter().all(|v| *v == 0) {
                    continue;
                }
                let n = u32::from_le_bytes(bytes[..4].try_into()?) as usize;
                ensure!(
                    n > 0
                        && n <= BYTES - 36
                        && ring::digest::digest(&ring::digest::SHA256, &bytes[36..36 + n]).as_ref()
                            == &bytes[4..36],
                    "Unknown task slot is not zero records"
                );
                count += 1;
            }
        }
    }
    Ok(count)
}
async fn body(mut response: reqwest::Response) -> Result<Vec<u8>> {
    ensure!(
        response
            .content_length()
            .is_none_or(|n| n <= MANAGED_RESPONSE_BYTES as u64),
        "Response length exceeds original protocol bound"
    );
    let mut bytes = Vec::new();
    while let Some(part) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("Response transport failed; no retry"))?
    {
        ensure!(
            part.len() <= MANAGED_RESPONSE_BYTES - bytes.len(),
            "Response exceeds original protocol bound"
        );
        bytes.extend_from_slice(&part);
    }
    Ok(bytes)
}
async fn request<T: serde::Serialize>(
    f: &ServiceFixture,
    route: &str,
    value: &T,
) -> Result<reqwest::Response> {
    f.client
        .post(format!("{}{route}", f.origin))
        .bearer_auth(&f.token)
        .json(value)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Native HTTPS transport failed; no retry"))
}
fn decoded<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("Invalid bounded response; body not exported"))
}
async fn observe(f: &ServiceFixture, slot: &Slot, report: &mut serde_json::Value) -> Result<()> {
    let began = Instant::now();
    report["request_ledger"] = serde_json::json!(["readiness"]);
    let ready = ManagedReadyRequest {
        format_version: 1,
        request_id: uuid::Uuid::new_v4().to_string(),
        environment: f.reference.clone(),
        target: f.target.clone(),
    };
    let response = request(f, "/v2/managed/ready", &ready).await?;
    ensure!(
        response.status().as_u16() == 200,
        "Readiness status differs"
    );
    let readiness: Signed<ManagedReadiness> = decoded(&body(response).await?)?;
    readiness.verify("managed_readiness", &f.binding.runner_public_key)?;
    readiness.payload.validate(&ready, &f.binding, wall())?;
    let data = b"approved HTTPS snapshot bytes\n";
    let manifest = cowork_domain::workspace_transfer::build(
        vec![],
        vec![WorkspaceTransferFile {
            path: "input.txt".into(),
            size: data.len() as u64,
            sha256: hash(data),
            chunks: vec![WorkspaceTransferChunk {
                offset: 0,
                size: data.len() as u64,
                sha256: hash(data),
            }],
        }],
    )?;
    let issued = wall();
    let invocation = ProcessInvocation {
        program: "probe".into(),
        arguments: vec![
            "copy".into(),
            path("COWORK_TEST_HOST_SENTINEL")
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Own sentinel encoding"))?
                .into(),
        ],
        timeout_seconds: 25,
    };
    let launch = f.control.sign(
        "launch",
        LaunchRequest {
            format_version: 1,
            runner_id: f.binding.runner_id.clone(),
            runner_generation: 1,
            adapter_build_sha256: f.binding.adapter_build_sha256.clone(),
            process_attempt_id: "service-attempt-copy".into(),
            run_id: "service-run-copy".into(),
            tool_call_id: "service-tool-copy".into(),
            request_sha256: hash(b"copy"),
            worker_fence_sha256: "d".repeat(64),
            lease_epoch: 1,
            environment: f.reference.clone(),
            workspace_resource_id: "not-input-authority".into(),
            invocation: invocation.clone(),
            issued_at: issued,
            expires_at: issued + 30000,
            execution_deadline: issued + 60000,
        },
    )?;
    let grant = f.control.sign(
        "managed_input_grant",
        ManagedInputGrant {
            format_version: 1,
            execution_plan_id: "fixture-confirmed-copy".into(),
            execution_plan_sha256: "e".repeat(64),
            target: f.target.clone(),
            runner: f.identity.clone(),
            input_manifest_sha256: manifest.sha256.clone(),
            process_attempt_id: launch.payload.process_attempt_id.clone(),
            run_id: launch.payload.run_id.clone(),
            request_sha256: launch.payload.request_sha256.clone(),
            environment: f.reference.clone(),
            invocation_sha256: canonical_hash(&invocation),
            worker_fence_sha256: launch.payload.worker_fence_sha256.clone(),
            lease_epoch: 1,
            issued_at: issued,
            expires_at: issued + 60000,
        },
    )?;
    let begin = ManagedBeginRequest {
        format_version: 1,
        readiness,
        launch,
        grant,
        manifest: manifest.clone(),
    };
    report["request_ledger"]
        .as_array_mut()
        .unwrap()
        .push("begin".into());
    let begin_started = Instant::now();
    let response = request(f, "/v2/managed/inputs", &begin).await?;
    ensure!(
        response.status().as_u16() == 201,
        "Original Begin status differs"
    );
    let ManagedBeginResponse::Input { ticket } = decoded(&body(response).await?)? else {
        anyhow::bail!("Original Begin did not return an input");
    };
    ticket.verify("managed_input_ticket", &f.binding.runner_public_key)?;
    ticket.payload.validate(&begin, wall())?;
    let input_hash = hash(ticket.payload.input_id.as_bytes());
    let attempt_hash = hash(begin.launch.payload.process_attempt_id.as_bytes());
    report["input_sha256"] = input_hash.clone().into();
    report["attempt_sha256"] = attempt_hash.clone().into();
    report["runtime_instance_sha256"] = ticket.payload.runtime_instance_sha256.clone().into();
    report["manifest_sha256"] = manifest.sha256.clone().into();
    report["ticket_sha256"] = canonical_hash(&ticket).into();
    report["original_expires_at"] = ticket.payload.expires_at.into();
    report["remaining_at_missing_send_millis"] = (ticket.payload.expires_at - wall()).into();
    report["request_ledger"]
        .as_array_mut()
        .unwrap()
        .push("missing".into());
    let missing_started = Instant::now();
    let response = request(
        f,
        &format!("/v2/managed/inputs/{}/missing", ticket.payload.input_id),
        &ManagedInputOperation {
            ticket: ticket.clone(),
        },
    )
    .await;
    let response_instant = Instant::now();
    let response_wall = wall();
    report["http_status"] = serde_json::json!(response.as_ref().ok().map(|r| r.status().as_u16()));
    // FIRST I/O after send resolves, for both HTTP and transport failure.
    report["diagnostic"] = slot.capture(&f.base, Some(&input_hash), Some(&attempt_hash))?;
    report["begin_to_response_millis"] =
        serde_json::json!(response_instant.duration_since(begin_started).as_millis());
    report["missing_elapsed_millis"] =
        serde_json::json!(response_instant.duration_since(missing_started).as_millis());
    report["readiness_to_response_millis"] =
        serde_json::json!(response_instant.duration_since(began).as_millis());
    report["remaining_at_missing_response_millis"] =
        (ticket.payload.expires_at - response_wall).into();
    let response = response?;
    let status = response.status().as_u16();
    report["http_status"] = status.into();
    let bytes = body(response).await?;
    report["response_bytes"] = bytes.len().into();
    report["response_sha256"] = hash(&bytes).into();
    if status == 200 {
        let missing: ManagedMissingResponse = decoded(&bytes)?;
        missing.validate(&ticket, &manifest)?;
        ticket.payload.validate(&begin, response_wall)?;
        ensure!(
            missing.missing.len() == 1,
            "Fresh original Missing list differs"
        );
        report["response_valid"] = true.into();
        report["classification"] = "single_component_success_F51_open".into();
    } else {
        let value: serde_json::Value = decoded(&bytes)?;
        let code = value.get("error").and_then(|v| v.as_str());
        ensure!(
            value.as_object().is_some_and(|v| v.len() == 1),
            "Non-allowlisted error body; not exported"
        );
        ensure!(
            matches!(
                (status, code),
                (409, Some("managed_execution_unconfirmed"))
                    | (403, Some("managed_authority_denied"))
            ),
            "Unclassified HTTP status/error; body not exported"
        );
        report["public_error_code"] = code.unwrap().into();
        let classification = if status == 403 {
            "authority_denied_not_historical409"
        } else if report["diagnostic"]["correlated"] == true {
            "new_correlated_missing_phase_original_F51_open"
        } else {
            "409_uncorrelated_F51_open"
        };
        report["classification"] = classification.into();
    }
    Ok(())
}

pub(super) async fn run(mut f: ServiceFixture) -> Result<()> {
    // reqwest0.12.28 otherwise retries some protocol NACKs by default. Only
    // this new observation disables them; the standard fixture is unchanged.
    f.client = reqwest::Client::builder()
        .no_proxy()
        .https_only(true)
        .add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(&f.ca)?)?)
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()?;
    let original_process = process_identity(&mut f.service)?;
    let slot = Slot::open(&f.state, &f.base)?;
    sqlx::any::install_default_drivers();
    let database = sqlx::any::AnyPoolOptions::new()
        .max_connections(1)
        .connect(&format!(
            "sqlite://{}?mode=ro",
            f.state.join("runner.db").display()
        ))
        .await?;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runner_executions")
        .fetch_one(&database)
        .await?;
    let mut report = serde_json::json!({"format_version":1,"fixture_only":true,"F51_closed":false,"http_status":null,
        "fixture_keys_and_qualification_are_synthetic":true,"profile_released":false,"runtime_qualified":false,"production_accepted":false,
        "classification":"setup_or_transport_unclassified","request_ledger":[],"response_valid":false,
        "service_original":original_process,"runner_executions_before":before,"harness_completed":false,
        "managed_operation_transport_retry":false,"shared_setup_unauthenticated_health_poll":true,
        "chunk_requests":0,"start_requests":0});
    let observation = if before == 0 {
        observe(&f, &slot, &mut report).await
    } else {
        Err(anyhow::anyhow!("Own fresh fixture already has executions"))
    };
    if !f.base.join(RAW).exists() {
        report["diagnostic"] = slot.capture(&f.base, None, None)?;
    }
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runner_executions")
        .fetch_one(&database)
        .await?;
    report["runner_executions_before_stop"] = after.into();
    let tasks_before = task_records(&f.case);
    report["task_records_before_stop"] = tasks_before
        .as_ref()
        .map_or(serde_json::Value::Null, |n| (*n).into());
    let same_process = process_identity(&mut f.service)
        .map(|v| v == original_process)
        .unwrap_or(false);
    report["original_process_unchanged_before_stop"] = same_process.into();
    let mut sigint_sent = false;
    let stopped = async {
        ensure!(
            same_process,
            "Original service identity changed before stop"
        );
        ensure!(
            unsafe { libc::kill(f.service.0.id() as i32, libc::SIGINT) } == 0,
            "Own service SIGINT failed"
        );
        sigint_sent = true;
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = f.service.0.try_wait()? {
                ensure!(status.success(), "Own service stop failed");
                return Ok::<_, anyhow::Error>(status);
            }
            ensure!(Instant::now() < until, "Own service stop deadline");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    .await;
    report["service_stop"] = serde_json::json!({"sigint_sent":sigint_sent,"reaped":stopped.is_ok(),"exit_code":stopped.as_ref().ok().and_then(|s|s.code())});
    let tasks_after = task_records(&f.case);
    report["task_records_after_stop"] = tasks_after
        .as_ref()
        .map_or(serde_json::Value::Null, |n| (*n).into());
    let pid = original_process["pid"].as_u64().unwrap() as u32;
    let absent =
        start_ticks(pid)?.is_none_or(|t| Some(t) != original_process["start_ticks"].as_u64());
    report["original_process_identity_absent_after_reap"] = absent.into();
    let after_stop: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runner_executions")
        .fetch_one(&database)
        .await?;
    report["runner_executions_after"] = after_stop.into();
    database.close().await;
    let complete = observation.is_ok()
        && stopped.is_ok()
        && absent
        && before == 0
        && after == 0
        && after_stop == 0
        && matches!(tasks_before, Ok(0))
        && matches!(tasks_after, Ok(0));
    report["harness_completed"] = complete.into();
    report["observation_error"] = observation.is_err().into();
    save(&f.base, REPORT, &serde_json::to_vec(&report)?)?;
    ensure!(
        complete,
        "Missing-once observation/cleanup failed; private bounded evidence retained"
    );
    println!("managed_https_missing_once=observed; chunks=0; start=0; F51_open=true");
    Ok(())
}
