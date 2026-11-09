//! Private test coordination only. Never dispatch, permission or stop evidence
//! accepted by product code. The fixture reads original native acknowledgements
//! and a guest-created marker before allowing the Controller test to request stop.
use super::*;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Armed {
    format_version: u32,
    run_id: String,
    process_attempt_id: String,
    request_sha256: String,
    execution_plan_sha256: String,
    input_manifest_sha256: String,
}

pub(super) struct Gate {
    case: PathBuf,
    controller: PathBuf,
    observed: bool,
    marker_in_alias: bool,
    completed: bool,
    original: Option<(Armed, PathBuf, String)>,
    client: reqwest::Client,
    origin: String,
    token: String,
    binding: Binding,
}
impl Gate {
    pub(super) fn new(
        case: &Path,
        controller: &Path,
        client: reqwest::Client,
        origin: String,
        token: String,
        binding: Binding,
        marker_in_alias: bool,
    ) -> Self {
        Self {
            case: case.into(),
            controller: controller.into(),
            observed: false,
            marker_in_alias,
            completed: false,
            original: None,
            client,
            origin,
            token,
            binding,
        }
    }
    pub(super) fn observed(&self) -> bool {
        self.observed
    }
    pub(super) fn completed(&self) -> bool {
        self.completed
    }
    pub(super) async fn poll_completion(&mut self) {
        if self.completed || !self.observed {
            return;
        }
        let (armed, control, task_id) = self.original.as_ref().unwrap();
        let response = self
            .client
            .get(format!(
                "{}/v2/receipts/{}",
                self.origin, armed.process_attempt_id
            ))
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(2))
            .send()
            .await;
        let Ok(response) = response else {
            return;
        };
        if response.status() != 200 {
            return;
        }
        let receipt: Signed<Receipt> = json(response, &[200]).await;
        receipt
            .verify("receipt", &self.binding.runner_public_key)
            .unwrap();
        assert_eq!(receipt.payload.request.runner_id, self.binding.runner_id);
        assert_eq!(
            receipt.payload.request.runner_generation,
            self.binding.generation
        );
        assert_eq!(
            receipt.payload.request.adapter_build_sha256,
            self.binding.adapter_build_sha256
        );
        assert_eq!(receipt.payload.request.run_id, armed.run_id);
        assert_eq!(
            receipt.payload.request.process_attempt_id,
            armed.process_attempt_id
        );
        assert_eq!(receipt.payload.request.request_sha256, armed.request_sha256);
        let ExecutionState::Completed { result } = &receipt.payload.state else {
            return;
        };
        assert_eq!(receipt.payload.revision, 3);
        assert_eq!(result.termination, ProcessTermination::Canceled);
        let Some((record, record_sha, record_bytes)) = latest_record(control) else {
            return;
        };
        assert_eq!(record["format_version"], 2);
        assert_eq!(record["task_id"], *task_id);
        assert_eq!(record["request_sha256"], armed.request_sha256);
        assert_eq!(record["plan_sha256"], armed.execution_plan_sha256);
        assert_eq!(record["input_manifest_sha256"], armed.input_manifest_sha256);
        if record["physical_stop_confirmed"] != true
            || record["control_process_reaped"] != true
            || record["output_complete"] != true
        {
            return;
        }
        let mutations = record["mutations"].as_array().unwrap();
        if mutations.len() != 12 || mutations.iter().any(|m| m["acknowledged"] != true) {
            return;
        }
        let outcome = frame(&control.join("outcome.bin"), 1024 * 1024)
            .expect("Original physical outcome framing");
        assert_eq!(record["outcome_sha256"], hash(&outcome));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&outcome).unwrap(),
            serde_json::to_value(result).unwrap()
        );
        if record["guest_exit_confirmed"] != true {
            assert!(
                result.exit_code.is_none(),
                "Unknown physical wait must stay nonnumeric"
            );
        }
        let value = serde_json::json!({"format_version":1,"diagnostic_only":true,
            "run_id":armed.run_id,"process_attempt_id":armed.process_attempt_id,
            "request_sha256":armed.request_sha256,"execution_receipt_sha256":canonical_hash(&receipt),
            "physical_record_sha256":record_sha});
        publish(
            &self.controller,
            c"controller-native-completed-record.json",
            &record_bytes,
        );
        publish(
            &self.controller,
            c"controller-native-completed.json",
            &serde_json::to_vec(&value).unwrap(),
        );
        self.completed = true;
    }
    pub(super) fn poll(&mut self) {
        if self.observed {
            return;
        }
        let armed_path = self.controller.join("controller-cancel-armed.json");
        if !armed_path.exists() {
            return;
        }
        let armed: Armed =
            serde_json::from_slice(&read_operator_file(&armed_path, 65536, true).unwrap()).unwrap();
        assert_eq!(armed.format_version, 1);
        assert!(uuid::Uuid::parse_str(&armed.run_id).is_ok());
        assert!(uuid::Uuid::parse_str(&armed.process_attempt_id).is_ok());
        for sha in [
            &armed.request_sha256,
            &armed.execution_plan_sha256,
            &armed.input_manifest_sha256,
        ] {
            assert!(cowork_domain::workspace_transfer::valid_sha256(sha));
        }
        let volumes = self.case.join("volumes");
        let entries = std::fs::read_dir(&volumes)
            .unwrap()
            .take(17)
            .collect::<std::io::Result<Vec<_>>>()
            .unwrap();
        assert!(entries.len() <= 16);
        for volume in &entries {
            let root = volume.path().join("root");
            let Some(children) =
                pending_volume_root(&root).expect("Native volume observation failed")
            else {
                continue;
            };
            let children = children
                .take(33)
                .collect::<std::io::Result<Vec<_>>>()
                .unwrap();
            assert!(children.len() <= 32);
            for child in children {
                if !child.file_name().to_str().unwrap().starts_with("cw-") {
                    continue;
                }
                let Some((record, record_sha, record_bytes)) = latest_record(&child.path()) else {
                    continue;
                };
                if record["request_sha256"] != armed.request_sha256
                    || record["plan_sha256"] != armed.execution_plan_sha256
                    || record["input_manifest_sha256"] != armed.input_manifest_sha256
                {
                    continue;
                }
                assert_eq!(record["format_version"], 2);
                assert_eq!(record["task_id"], child.file_name().to_str().unwrap());
                if record["physical_stop_confirmed"] != false
                    || record["task_pid"].as_u64().is_none_or(|pid| pid == 0)
                    || !record["mutations"].as_array().unwrap().iter().any(|m| {
                        m["operation"] == "task_start"
                            && m["container_id"] == record["task_id"]
                            && m["acknowledged"] == true
                            && m["response"] == "succeeded"
                    })
                {
                    continue;
                }
                let volume_id = record["volume_id"].as_str().unwrap();
                assert!(uuid::Uuid::parse_str(volume_id).is_ok());
                let work_root = volumes.join(volume_id).join("root");
                let intent: serde_json::Value = serde_json::from_slice(
                    &read_operator_file(
                        &work_root.join("transfer/intent.json"),
                        3 * 1024 * 1024,
                        true,
                    )
                    .unwrap(),
                )
                .unwrap();
                let manifest: WorkspaceTransferManifest =
                    serde_json::from_value(intent["manifest"].clone()).unwrap();
                cowork_domain::workspace_transfer::validate(&manifest).unwrap();
                assert_eq!(manifest.sha256, armed.input_manifest_sha256);
                let source_files = manifest
                    .files
                    .iter()
                    .filter(|f| {
                        Path::new(&f.path)
                            .file_name()
                            .is_some_and(|n| n == "value.txt")
                    })
                    .collect::<Vec<_>>();
                assert_eq!(source_files.len(), 1);
                assert_eq!(
                    source_files[0].sha256,
                    hash(b"approved HTTPS controller snapshot bytes\n")
                );
                let output = Path::new(&source_files[0].path).with_file_name("result.txt");
                let workspace = work_root.join("work/workspace");
                let marker_path = if self.marker_in_alias {
                    output.with_file_name("started")
                } else {
                    PathBuf::from("started")
                };
                let Some(marker) = guest_file(&workspace, &marker_path) else {
                    continue;
                };
                let Some(result) = guest_file(&workspace, &output) else {
                    continue;
                };
                if marker != b"started" || result != b"approved HTTPS controller snapshot bytes\n" {
                    continue;
                }
                let value = serde_json::json!({"format_version":1,"diagnostic_only":true,
                    "run_id":armed.run_id,"process_attempt_id":armed.process_attempt_id,
                    "request_sha256":armed.request_sha256,"task_id":record["task_id"],
                    "physical_record_sha256":record_sha,"guest_marker_sha256":hash(&marker)});
                publish(
                    &self.controller,
                    c"controller-native-start-record.json",
                    &record_bytes,
                );
                publish(
                    &self.controller,
                    c"controller-native-start.json",
                    &serde_json::to_vec(&value).unwrap(),
                );
                self.original = Some((
                    armed,
                    child.path(),
                    record["task_id"].as_str().unwrap().to_owned(),
                ));
                self.observed = true;
                return;
            }
        }
    }
}

fn pending_volume_root(root: &Path) -> std::io::Result<Option<std::fs::ReadDir>> {
    // Reservation creates its journal directory before it creates the root.
    // Missing root is not a native start observation; the bounded caller polls
    // again. Every other filesystem error remains a failed fixture.
    match std::fs::read_dir(root) {
        Ok(entries) => Ok(Some(entries)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[test]
fn native_start_poll_waits_for_uncreated_volume_root_and_rejects_other_errors() {
    let fixture = tempfile::tempdir().unwrap();
    let reservation = fixture.path().join("reservation");
    std::fs::create_dir(&reservation).unwrap();
    let root = reservation.join("root");
    assert!(pending_volume_root(&root).unwrap().is_none());
    std::fs::create_dir(&root).unwrap();
    let mut observed = pending_volume_root(&root).unwrap().unwrap();
    assert!(observed.next().is_none());
    drop(observed);
    let wrong_type = reservation.join("file");
    std::fs::write(&wrong_type, b"not a volume directory").unwrap();
    assert_eq!(
        pending_volume_root(&wrong_type).unwrap_err().raw_os_error(),
        Some(libc::ENOTDIR)
    );
    fixture.close().unwrap();
}

pub(super) fn frame(path: &Path, bytes: usize) -> Option<Vec<u8>> {
    let data = read_operator_file(path, bytes as u64, true).ok()?;
    assert_eq!(data.len(), bytes);
    let length = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
    // A concurrent slot replacement is not evidence. It must qualify on a
    // later poll or through the other original, fully verified journal slot.
    if length == 0 || length > bytes - 36 {
        return None;
    }
    let payload = &data[36..36 + length];
    if ring::digest::digest(&ring::digest::SHA256, payload).as_ref() != &data[4..36]
        || data[36 + length..].iter().any(|b| *b != 0)
    {
        return None;
    }
    Some(payload.to_vec())
}
pub(super) fn latest_record(control: &Path) -> Option<(serde_json::Value, String, Vec<u8>)> {
    ["task.a", "task.b"]
        .into_iter()
        .filter_map(|name| {
            let bytes = frame(&control.join(name), 16384)?;
            Some((
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                hash(&bytes),
                bytes,
            ))
        })
        .max_by_key(|(r, _, _)| r["sequence"].as_u64().unwrap())
}

fn guest_file(workspace: &Path, relative: &Path) -> Option<Vec<u8>> {
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::OpenOptionsExt;
    let mut directory =
        cap_std::fs::Dir::open_ambient_dir(workspace, cap_std::ambient_authority()).unwrap();
    let parts = relative.components().collect::<Vec<_>>();
    for part in &parts[..parts.len() - 1] {
        let std::path::Component::Normal(name) = part else {
            panic!("Invalid guest path");
        };
        directory = directory.open_dir_nofollow(name).unwrap();
    }
    let std::path::Component::Normal(name) = parts.last().unwrap() else {
        panic!("Invalid guest filename");
    };
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    let file = match directory.open_with(name, &options) {
        Ok(file) => file.into_std(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => panic!("Guest marker cannot be verified: {e}"),
    };
    let metadata = file.metadata().unwrap();
    assert!(
        metadata.is_file()
            && metadata.uid() == 1000
            && metadata.nlink() == 1
            && metadata.len() <= 4096
    );
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes).unwrap();
    assert!(bytes.len() <= 4096);
    Some(bytes)
}

pub(super) fn publish(controller: &Path, name: &std::ffi::CStr, bytes: &[u8]) {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    let temporary = format!("native-gate-{}.tmp", uuid::Uuid::new_v4());
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(controller.join(&temporary))
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(controller)
        .unwrap();
    let from = std::ffi::CString::new(temporary).unwrap();
    let result = unsafe {
        libc::renameat2(
            directory.as_raw_fd(),
            from.as_ptr(),
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    assert_eq!(result, 0, "Native test gate must publish exactly once");
    directory.sync_all().unwrap();
}
