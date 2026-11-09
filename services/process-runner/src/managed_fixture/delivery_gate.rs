//! UID0 test observer only. Its gates authorize no product execution or cleanup.
//! Original native frames are captured before the Controller can confirm ACK.
use super::*;
use cowork_managed_runtime::volumes::{PinnedFormatter, VolumePool};
use std::os::{fd::AsRawFd, unix::fs::MetadataExt};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    format_version: u32,
    index: usize,
    receipt: Signed<Receipt>,
    attestation: Signed<ManagedOutputAttestation>,
    manifest_sha256: String,
    result_version_id: String,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Retired {
    format_version: u32,
    index: usize,
    run_id: String,
    process_attempt_id: String,
    result_version_id: String,
    #[serde(rename = "delivery_receipt")]
    delivery: Signed<ManagedResultDeliveryReceipt>,
    #[serde(rename = "retirement_receipt")]
    retirement: Signed<ManagedResultRetirementReceipt>,
}
pub(super) struct Gate {
    case: PathBuf,
    controller: PathBuf,
    binding: Binding,
    pending: Option<serde_json::Value>,
    observations: Vec<serde_json::Value>,
}
impl Gate {
    pub(super) fn new(case: &Path, controller: &Path, binding: Binding) -> Self {
        Self {
            case: case.into(),
            controller: controller.into(),
            binding,
            pending: None,
            observations: vec![],
        }
    }
    pub(super) fn completed(&self) -> bool {
        self.observations.len() == 2
    }
    pub(super) fn observations(&self) -> &[serde_json::Value] {
        &self.observations
    }
    fn publish(&self, name: &str, value: &serde_json::Value) {
        let name = std::ffi::CString::new(name).unwrap();
        cancel_gate::publish(&self.controller, &name, &serde_json::to_vec(value).unwrap());
    }
    fn pool(&self) -> VolumePool {
        let formatter = Path::new("/usr/sbin/mke2fs").canonicalize().unwrap();
        let digest = hash(&std::fs::read(&formatter).unwrap());
        VolumePool::open(
            cap_std::fs::Dir::open_ambient_dir(&self.case, cap_std::ambient_authority()).unwrap(),
            PinnedFormatter::open(&formatter, &digest).unwrap(),
        )
        .unwrap()
    }
    pub(super) fn poll(&mut self) {
        if self.completed() {
            return;
        }
        let index = self.observations.len() + 1;
        if self.pending.is_none() {
            let path = self
                .controller
                .join(format!("controller-delivery-ready-{index}.json"));
            if !path.exists() {
                return;
            }
            let ready: Ready =
                serde_json::from_slice(&read_operator_file(&path, 65536, true).unwrap()).unwrap();
            self.capture(index, ready);
        }
        let path = self
            .controller
            .join(format!("controller-delivery-retired-{index}.json"));
        if !path.exists() {
            return;
        }
        let retired: Retired =
            serde_json::from_slice(&read_operator_file(&path, 65536, true).unwrap()).unwrap();
        assert_eq!(retired.format_version, 1);
        assert_eq!(retired.index, index);
        let pending = self.pending.as_ref().unwrap();
        retired
            .delivery
            .verify(
                "managed_result_delivery_receipt",
                &self.binding.runner_public_key,
            )
            .unwrap();
        retired
            .delivery
            .payload
            .validate(&retired.delivery.payload.request, &self.binding)
            .unwrap();
        retired
            .retirement
            .verify(
                "managed_result_retirement_receipt",
                &self.binding.runner_public_key,
            )
            .unwrap();
        retired
            .retirement
            .payload
            .validate(
                &retired.delivery,
                &self.binding,
                pending["physical_record_sha256"].as_str().unwrap(),
            )
            .unwrap();
        let request = &retired.delivery.payload.request.payload;
        assert_eq!(retired.run_id, request.run_id);
        assert_eq!(retired.process_attempt_id, request.process_attempt_id);
        assert_eq!(retired.result_version_id, request.recipient_version_id);
        assert_eq!(
            request.execution_receipt_sha256,
            pending["execution_receipt_sha256"]
        );
        assert_eq!(
            request.output_attestation_sha256,
            pending["output_attestation_sha256"]
        );
        assert_eq!(request.manifest_sha256, pending["manifest_sha256"]);
        assert_eq!(request.recipient_version_id, pending["result_version_id"]);
        assert_eq!(
            retired.retirement.payload.state,
            ManagedResultRetirementState::Retired
        );
        let Ok(usage) = self.pool().usage() else {
            return;
        };
        if usage.reserved_bytes != 0 || usage.reserved_volumes != 0 || usage.retained_records != 0 {
            return;
        }
        for id in ["workspace_volume_id", "control_volume_id"] {
            assert!(
                matches!(std::fs::symlink_metadata(self.case.join("volumes").join(pending[id].as_str().unwrap())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound)
            );
        }
        let available = available(&self.case);
        let before = pending["outer_available_before_ack"].as_u64().unwrap();
        let allocated = pending["original_image_allocated_bytes"].as_u64().unwrap();
        assert!(
            available >= before + allocated - 2 * 1024 * 1024,
            "Original image blocks must really return; small Controller/journal growth remains separate"
        );
        let mut observation = self.pending.take().unwrap();
        observation["delivery"] = serde_json::to_value(retired.delivery).unwrap();
        observation["retirement"] = serde_json::to_value(retired.retirement).unwrap();
        observation["pool_after"] = serde_json::to_value(usage).unwrap();
        observation["outer_available_after_gc"] = available.into();
        observation["original_namespaces_absent"] = true.into();
        observation["collected"] = true.into();
        self.publish(
            &format!("native-delivery-collected-{index}.json"),
            &observation,
        );
        self.observations.push(observation);
    }
    fn capture(&mut self, index: usize, ready: Ready) {
        assert_eq!(ready.format_version, 1);
        assert_eq!(ready.index, index);
        assert!(uuid::Uuid::parse_str(&ready.result_version_id).is_ok());
        ready
            .receipt
            .verify("receipt", &self.binding.runner_public_key)
            .unwrap();
        assert_eq!(
            ready.receipt.payload.request.runner_id,
            self.binding.runner_id
        );
        assert_eq!(
            ready.receipt.payload.request.runner_generation,
            self.binding.generation
        );
        assert_eq!(
            ready.receipt.payload.request.adapter_build_sha256,
            self.binding.adapter_build_sha256
        );
        ready
            .attestation
            .verify("managed_output", &self.binding.runner_public_key)
            .unwrap();
        let ExecutionState::Completed { result } = &ready.receipt.payload.state else {
            panic!("Original completed receipt required")
        };
        assert_eq!(result.termination, ProcessTermination::Exited);
        assert_eq!(result.exit_code, Some(0));
        let retained = recovery(&self.case, &ready.receipt).unwrap();
        assert_eq!(
            retained["attestation"],
            serde_json::to_value(&ready.attestation).unwrap()
        );
        assert_eq!(retained["manifest"]["sha256"], ready.manifest_sha256);
        let physical_hash = &ready.attestation.payload.physical_record_sha256;
        let mut originals = vec![];
        for entry in std::fs::read_dir(self.case.join("volumes")).unwrap() {
            let volume = entry.unwrap().path();
            for entry in std::fs::read_dir(volume.join("root")).unwrap() {
                let control = entry.unwrap().path();
                if !control
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("cw-")
                {
                    continue;
                }
                if let Some((record, _, bytes)) = cancel_gate::latest_record(&control)
                    && canonical_hash(&record) == *physical_hash
                {
                    originals.push((control, record, bytes));
                }
            }
        }
        assert_eq!(originals.len(), 1);
        let (control, record, bytes) = originals.pop().unwrap();
        assert_eq!(record["format_version"], 2);
        assert_eq!(
            record["request_sha256"],
            ready.receipt.payload.request.request_sha256
        );
        assert_eq!(
            record["plan_sha256"],
            ready.attestation.payload.execution_plan_sha256
        );
        assert_eq!(
            record["input_manifest_sha256"],
            ready.attestation.payload.input_manifest_sha256
        );
        for key in [
            "physical_stop_confirmed",
            "control_process_reaped",
            "output_complete",
            "guest_exit_confirmed",
        ] {
            assert_eq!(record[key], true);
        }
        let mutations = record["mutations"].as_array().unwrap();
        assert_eq!(mutations.len(), 12);
        assert!(
            mutations
                .iter()
                .all(|item| item["acknowledged"] == true && !item["response"].is_null())
        );
        let outcome = cancel_gate::frame(&control.join("outcome.bin"), 1024 * 1024).unwrap();
        assert_eq!(record["outcome_sha256"], hash(&outcome));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&outcome).unwrap(),
            serde_json::to_value(result).unwrap()
        );
        for (name, maximum) in [
            ("task.a", 16384),
            ("task.b", 16384),
            ("outcome.bin", 1024 * 1024),
        ] {
            let raw = read_operator_file(&control.join(name), maximum, true).unwrap();
            assert_eq!(raw.len() as u64, maximum);
            let name =
                std::ffi::CString::new(format!("delivery-capture-{index}-{name}.bin")).unwrap();
            cancel_gate::publish(&self.controller, &name, &raw);
        }
        let mut allocated = 0;
        for id in [
            record["volume_id"].as_str().unwrap(),
            record["control_volume_id"].as_str().unwrap(),
        ] {
            assert!(uuid::Uuid::parse_str(id).is_ok());
            let image = self.case.join("volumes").join(id).join("image.ext4");
            let metadata = std::fs::symlink_metadata(image).unwrap();
            assert!(metadata.is_file() && metadata.uid() == 0 && metadata.nlink() == 1);
            allocated += metadata.blocks() * 512;
        }
        let usage = self.pool().usage().unwrap();
        assert_eq!(usage.reserved_bytes, 48 * 1024 * 1024);
        assert_eq!(usage.reserved_volumes, 2);
        assert_eq!(usage.retained_records, 2);
        let observation = serde_json::json!({"format_version":1,"diagnostic_only":true,"index":index,
            "execution_receipt_sha256":canonical_hash(&ready.receipt), "output_attestation_sha256":canonical_hash(&ready.attestation),
            "manifest_sha256":ready.manifest_sha256,"result_version_id":ready.result_version_id,
            "physical_record_sha256":physical_hash,"physical_record":record,"physical_record_payload_sha256":hash(&bytes),
            "workspace_volume_id":record["volume_id"],"control_volume_id":record["control_volume_id"],
            "original_image_allocated_bytes":allocated,"outer_available_before_ack":available(&self.case),"pool_before":usage,
            "receipt":ready.receipt,"native_recovery":retained});
        self.publish(
            &format!("native-delivery-captured-{index}.json"),
            &observation,
        );
        self.pending = Some(observation);
    }
}
fn available(path: &Path) -> u64 {
    let file = File::open(path).unwrap();
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    assert_eq!(
        unsafe { libc::fstatvfs(file.as_raw_fd(), info.as_mut_ptr()) },
        0
    );
    let info = unsafe { info.assume_init() };
    info.f_bavail.checked_mul(info.f_frsize).unwrap()
}
