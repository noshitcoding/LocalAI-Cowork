//! Real HTTPS delivery and local pack verification. File gates are QA observers,
//! never native cleanup authority or substitutes for signed product receipts.
use super::*;
use cowork_runner_protocol::{ManagedResultManifest, Receipt, Signed};
use cowork_storage::ManagedResultDeliveryState;

pub(super) struct Case<'a> {
    pub root: &'a Path,
    pub index: usize,
    pub runtime: &'a Runtime,
    pub context: &'a ControllerContext,
    pub run: &'a str,
    pub attempt: &'a str,
    pub receipt: &'a Signed<Receipt>,
    pub result: &'a ManagedResultManifest,
    pub version: &'a str,
}

async fn gate(root: &Path, name: &str) -> Result<Value> {
    let path = root.join(name);
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        match std::fs::symlink_metadata(&path) {
            Ok(_) => return read_private(&path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        ensure!(
            Instant::now() < deadline,
            "Native fixture observation gate timed out: {name}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(super) async fn confirm(case: Case<'_>) -> Result<()> {
    let Case {
        root,
        index,
        runtime,
        context,
        run,
        attempt,
        receipt,
        result,
        version,
    } = case;
    write_private_diagnostic(
        root,
        &format!("controller-delivery-ready-{index}.json"),
        &json!({"format_version":1,"index":index,"receipt":receipt,
            "attestation":result.attestation,"manifest_sha256":result.manifest.sha256,
            "result_version_id":version}),
    )?;
    let captured = gate(root, &format!("native-delivery-captured-{index}.json")).await?;
    ensure!(
        captured["format_version"] == 1
            && captured["index"] == index
            && captured["execution_receipt_sha256"] == canonical_hash(receipt)
            && captured["physical_record_sha256"]
                == result.attestation.payload.physical_record_sha256,
        "Native capture did not bind the original completion"
    );
    // No 30-second confirmation window is spent waiting for the QA observer.
    let prepared = runtime
        .prepare_owner_managed_result_delivery(&context.order_id, run, attempt)
        .await?;
    ensure!(
        prepared.state == ManagedResultDeliveryState::Prepared
            && prepared.result_version_id == version,
        "Fresh delivery preparation did not bind local Ready"
    );
    let accepted = runtime
        .confirm_owner_managed_result_delivery(
            &context.order_id,
            run,
            attempt,
            &prepared.id,
            &canonical_hash(&prepared.request),
        )
        .await?;
    let original = accepted.accepted.context("Original delivery ACK missing")?;
    let deadline = Instant::now() + Duration::from_secs(90);
    let retired = loop {
        let current = runtime
            .refresh_owner_managed_result_retirement(&context.order_id, run, attempt)
            .await?;
        ensure!(
            current.accepted.as_ref() == Some(&original),
            "Original ACK changed during status refresh"
        );
        if current.state == ManagedResultDeliveryState::Retired {
            break current;
        }
        ensure!(
            current.state != ManagedResultDeliveryState::Unconfirmed,
            "Native retirement became unconfirmed"
        );
        ensure!(
            Instant::now() < deadline,
            "Physical retirement did not settle"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    write_private_diagnostic(
        root,
        &format!("controller-delivery-retired-{index}.json"),
        &json!({"format_version":1,"index":index,"run_id":run,"process_attempt_id":attempt,
            "result_version_id":version,"delivery_receipt":original,
            "retirement_receipt":retired.retirement.context("Signed retirement missing")?}),
    )?;
    let collected = gate(root, &format!("native-delivery-collected-{index}.json")).await?;
    ensure!(
        collected["format_version"] == 1
            && collected["index"] == index
            && collected["collected"] == true
            && collected["execution_receipt_sha256"] == canonical_hash(receipt)
            && collected["physical_record_sha256"]
                == result.attestation.payload.physical_record_sha256
            && collected["original_namespaces_absent"] == true
            && collected["pool_after"]["reserved_bytes"] == 0
            && collected["pool_after"]["reserved_volumes"] == 0
            && collected["pool_after"]["retained_records"] == 0,
        "Native pool did not return to its original empty baseline"
    );
    Ok(())
}

pub(super) struct LocalResult {
    pub order: String,
    pub run: String,
    pub attempt: String,
    pub version: String,
    pub manifest: WorkspaceTransferManifest,
    pub expected: Vec<u8>,
    pub original_directory: PathBuf,
    pub folder_id: String,
}
impl LocalResult {
    pub(super) async fn verify(&self, runtime: &Runtime) -> Result<()> {
        let file = self
            .manifest
            .files
            .iter()
            .find(|f| f.path.ends_with("/result.txt"))
            .context("Original declared result missing")?;
        let mut bytes = Vec::new();
        for chunk in &file.chunks {
            let value = runtime
                .managed_result_version_chunk(
                    &self.order,
                    &self.run,
                    &self.attempt,
                    &self.version,
                    ReadWorkspaceTransferChunk {
                        path: file.path.clone(),
                        offset: chunk.offset,
                        expected_sha256: chunk.sha256.clone(),
                    },
                )
                .await?;
            ensure!(
                value.len() as u64 == chunk.size && hash(&value) == chunk.sha256,
                "Local result chunk changed after native GC"
            );
            bytes.extend_from_slice(&value);
        }
        ensure!(
            bytes == self.expected && hash(&bytes) == file.sha256,
            "Local result changed after native GC"
        );
        ensure!(
            std::fs::read(self.original_directory.join("value.txt"))? == self.expected
                && !self.original_directory.join("result.txt").exists(),
            "Original source changed after native GC"
        );
        ensure!(
            runtime
                .run_copy_text(
                    &self.order,
                    &self.run,
                    FileLocation {
                        folder_id: self.folder_id.clone(),
                        path: "value.txt".into(),
                    },
                )
                .await?
                .content
                .as_bytes()
                == self.expected,
            "Confirmed local copy changed after native GC"
        );
        ensure!(
            matches!(
                runtime
                    .run_copy_text(
                        &self.order,
                        &self.run,
                        FileLocation {
                            folder_id: self.folder_id.clone(),
                            path: "result.txt".into(),
                        },
                    )
                    .await,
                Err(cowork_domain::Error::NotFound)
            ),
            "Managed output was implicitly adopted after native GC"
        );
        Ok(())
    }
}
