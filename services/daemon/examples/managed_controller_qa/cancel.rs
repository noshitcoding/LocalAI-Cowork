//! Test coordination for a genuinely started blocking guest. Gate files are
//! diagnostic only; the product adapter independently verifies original evidence.
use super::*;

#[cfg(target_os = "linux")]
fn publish(root: &Path, name: &str, value: &Value) -> Result<()> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    let temporary = format!("controller-gate-{}.tmp", uuid::Uuid::new_v4());
    write_private_diagnostic(root, &temporary, value)?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let from = std::ffi::CString::new(temporary)?;
    let to = std::ffi::CString::new(name)?;
    let result = unsafe {
        libc::renameat2(
            directory.as_raw_fd(),
            from.as_ptr(),
            directory.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    directory.sync_all()?;
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn publish(_: &Path, _: &str, _: &Value) -> Result<()> {
    anyhow::bail!("Native cancellation coordination requires the Linux fixture")
}

async fn gate(root: &Path, name: &str, deadline: Instant) -> Result<Value> {
    loop {
        match std::fs::symlink_metadata(root.join(name)) {
            Ok(_) => return read_private(&root.join(name)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        ensure!(
            Instant::now() < deadline,
            "Native diagnostic gate missing: {name}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
fn bound_gate(value: &Value, run: &str, attempt: &str, request: &str) -> Result<()> {
    ensure!(
        value["format_version"] == 1
            && value["diagnostic_only"] == true
            && value["run_id"] == run
            && value["process_attempt_id"] == attempt
            && value["request_sha256"] == request
            && value["physical_record_sha256"]
                .as_str()
                .is_some_and(cowork_domain::processes::valid_sha256),
        "Native coordination gate does not bind the original execution"
    );
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "Cancellation qualification keeps the accepted run, prepared plan, original confirmation, source bytes and model fixture explicit with the runtime, store, controller and handoff."
)]
pub(super) async fn run(
    root: &Path,
    runtime: &Runtime,
    store: &Store,
    context: &ControllerContext,
    handoff: &Handoff,
    accepted: &RunRecord,
    prepared: &PreparedExecutionPlanV2,
    confirmation: &CommitExecutionPlan,
    source_before: &[u8],
    model: &ModelFixture,
) -> Result<()> {
    let stop_read = handoff.mode == "owner_stop_read";
    let deadline = Instant::now() + Duration::from_secs(100);
    let files = runtime
        .files
        .clone()
        .context("Controller FileBroker missing")?;
    let origin = tokio::task::spawn_blocking(move || files.local_controller_origin()).await??;
    let history = loop {
        ensure!(
            Instant::now() < deadline,
            "Original managed input was not prepared"
        );
        let attempts = store.process_attempts(&accepted.id).await?;
        ensure!(
            attempts.len() <= 1,
            "Blocking fixture dispatched more than once"
        );
        if let Some(attempt) = attempts.first() {
            match store
                .owner_managed_process_history_context(
                    &context.order_id,
                    &accepted.id,
                    &attempt.id,
                    &origin,
                )
                .await
            {
                Ok(history) => break history,
                Err(cowork_domain::Error::NotFound) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let run = store.stored_run(&accepted.id).await?;
        ensure!(
            !matches!(
                run.record.state,
                RunState::Failed
                    | RunState::Interrupted
                    | RunState::Completed
                    | RunState::Canceled
                    | RunState::Expired
            ),
            "Original worker settled before native startup: {:?}, {:?}",
            run.record.state,
            run.record.error_code
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let attempt = &history.process_attempt;
    ensure!(
        attempt.state == ProcessAttemptState::LaunchCommitted && history.receipt.is_none(),
        "Expected one uncompleted original launch intent"
    );
    publish(
        root,
        "controller-cancel-armed.json",
        &json!({"format_version":1,
        "run_id":accepted.id,"process_attempt_id":attempt.id,"request_sha256":attempt.request_sha256,
        "execution_plan_sha256":history.input.grant.payload.execution_plan_sha256,
        "input_manifest_sha256":history.input.grant.payload.input_manifest_sha256}),
    )?;
    let started = gate(root, "controller-native-start.json", deadline).await?;
    bound_gate(&started, &accepted.id, &attempt.id, &attempt.request_sha256)?;
    ensure!(
        started["guest_marker_sha256"] == hash(b"started")
            && started["task_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("cw-")),
        "Original guest did not publish its started marker"
    );
    // This is the actual product cancellation mutation, after native task_start
    // acknowledgement and guest execution, never direct token cancellation.
    let canceled = store.cancel(&accepted.id).await?;
    ensure!(
        canceled.state == RunState::CancelRequested,
        "Expected actual running cancellation"
    );
    let interrupted = loop {
        let saved = store.stored_run(&accepted.id).await?;
        if saved.record.state == RunState::Interrupted && saved.lease.is_none() {
            break saved;
        }
        ensure!(
            Instant::now() < deadline
                && !matches!(
                    saved.record.state,
                    RunState::Completed | RunState::Failed | RunState::Canceled | RunState::Expired
                ),
            "Canceled worker did not retain the uncertain original attempt: {:?}",
            saved.record.state
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    ensure!(
        interrupted.checkpoint.in_flight.as_ref() == Some(&attempt.tool),
        "Uncertain process was silently settled before physical evidence"
    );
    let completed = gate(root, "controller-native-completed.json", deadline).await?;
    bound_gate(
        &completed,
        &accepted.id,
        &attempt.id,
        &attempt.request_sha256,
    )?;
    let intent = ReconcileOwnerManagedProcess {
        expected_run_revision: interrupted.record.revision,
        expected_request_sha256: attempt.request_sha256.clone(),
        finish_cancellation: !stop_read,
    };
    let settled = runtime
        .reconcile_owner_managed_process(
            &context.order_id,
            &accepted.id,
            &attempt.id,
            intent.clone(),
        )
        .await?;
    ensure!(
        settled.run.state
            == if stop_read {
                RunState::Interrupted
            } else {
                RunState::Canceled
            }
            && matches!(&settled.process_attempt.state, ProcessAttemptState::ReconciledCompleted {result,..} if result.termination == ProcessTermination::Canceled),
        "Business cancellation and actual physical cancellation were not independently retained"
    );
    let receipt = store
        .managed_completion_receipt(&accepted.id, &attempt.id)
        .await?
        .context("Original signed physical completion was not retained")?;
    receipt.verify("receipt", &handoff.binding.runner_public_key)?;
    ensure!(
        completed["execution_receipt_sha256"] == canonical_hash(&receipt),
        "Native completion and original receipt differ"
    );
    let events = store.events(&accepted.id, 0).await?;
    let replay = runtime
        .reconcile_owner_managed_process(&context.order_id, &accepted.id, &attempt.id, intent)
        .await?;
    ensure!(
        canonical_hash(&replay) == canonical_hash(&settled)
            && canonical_hash(&store.events(&accepted.id, 0).await?) == canonical_hash(&events),
        "Exact reconciliation replay changed the original settlement"
    );
    ensure!(
        store
            .stored_run(&accepted.id)
            .await?
            .checkpoint
            .in_flight
            .is_none()
            && store.process_attempts(&accepted.id).await?.len() == 1
            && store.runs(&context.order_id).await?.len() == 1
            && model.requests.load(Ordering::SeqCst) == 1
            && runtime
                .commit_execution_plan_v2(&context.order_id, &prepared.plan.id, confirmation)
                .await?
                .id
                == accepted.id,
        "Cancellation recovery redispatched or continued the model"
    );
    ensure!(
        std::fs::read(context.original_directory.join("value.txt"))? == source_before
            && !context.original_directory.join("result.txt").exists(),
        "Original input changed"
    );
    let copy = runtime
        .run_copy_text(
            &context.order_id,
            &accepted.id,
            FileLocation {
                folder_id: context.folder_id.clone(),
                path: "value.txt".into(),
            },
        )
        .await?;
    ensure!(
        copy.content.as_bytes() == source_before,
        "Confirmed local input copy changed"
    );
    ensure!(
        matches!(
            runtime
                .run_copy_text(
                    &context.order_id,
                    &accepted.id,
                    FileLocation {
                        folder_id: context.folder_id.clone(),
                        path: "result.txt".into()
                    }
                )
                .await,
            Err(cowork_domain::Error::NotFound)
        ),
        "Canceled remote output was implicitly adopted"
    );
    let (result_version_id, manifest_sha256) = if stop_read {
        let id = runtime
            .retain_managed_result_version(&context.order_id, &accepted.id, &attempt.id)
            .await?;
        let manifest = runtime
            .owner_managed_result_manifest(&context.order_id, &accepted.id, &id)
            .await?;
        let input = history
            .input
            .manifest
            .files
            .iter()
            .find(|file| {
                file.path
                    .ends_with(&format!("/{}", handoff.input_file_name))
            })
            .context("Original declared fixture input missing")?;
        let (alias, _) = input
            .path
            .split_once('/')
            .context("Declared fixture alias missing")?;
        let result_path = format!("{alias}/{}", handoff.output_file_name);
        let result = manifest
            .manifest
            .files
            .iter()
            .find(|file| file.path == result_path)
            .context("Stopped guest result is absent from original output")?;
        let mut bytes = Vec::new();
        for chunk in &result.chunks {
            let read = runtime
                .owner_managed_result_chunk(
                    &context.order_id,
                    &accepted.id,
                    &id,
                    ReadWorkspaceTransferChunk {
                        path: result.path.clone(),
                        offset: chunk.offset,
                        expected_sha256: chunk.sha256.clone(),
                    },
                )
                .await?;
            bytes.extend_from_slice(&read.bytes);
        }
        ensure!(
            bytes == source_before && hash(&bytes) == result.sha256,
            "Fresh post-stop result differs from the actual original guest output"
        );
        ensure!(
            runtime
                .retain_managed_result_version(&context.order_id, &accepted.id, &attempt.id)
                .await?
                == id,
            "Local Ready replay changed the result version"
        );
        let after = store.stored_run(&accepted.id).await?;
        ensure!(
            after.lease.is_none()
                && after.record.state == RunState::Interrupted
                && after.record.revision == settled.run.revision
                && store.process_attempts(&accepted.id).await?.len() == 1
                && model.requests.load(Ordering::SeqCst) == 1
                && canonical_hash(
                    &store
                        .managed_completion_receipt(&accepted.id, &attempt.id)
                        .await?
                        .context("Original receipt disappeared")?
                ) == canonical_hash(&receipt),
            "Reading a stopped original revived execution or changed completion history"
        );
        // Check preservation after the newly added receive and local reads too.
        ensure!(
            std::fs::read(context.original_directory.join("value.txt"))? == source_before
                && !context.original_directory.join("result.txt").exists(),
            "Receiving a stopped result changed the original files"
        );
        let after_copy = runtime
            .run_copy_text(
                &context.order_id,
                &accepted.id,
                FileLocation {
                    folder_id: context.folder_id.clone(),
                    path: "value.txt".into(),
                },
            )
            .await?;
        ensure!(
            after_copy.content.as_bytes() == source_before,
            "Receiving a stopped result changed the confirmed local input copy"
        );
        ensure!(
            matches!(
                runtime
                    .run_copy_text(
                        &context.order_id,
                        &accepted.id,
                        FileLocation {
                            folder_id: context.folder_id.clone(),
                            path: "result.txt".into()
                        }
                    )
                    .await,
                Err(cowork_domain::Error::NotFound)
            ),
            "Received remote output was implicitly adopted into the confirmed input copy"
        );
        (Some(id), Some(manifest.manifest.sha256))
    } else {
        (None, None)
    };
    write_private_diagnostic(
        root,
        "controller-result.json",
        &json!({"format_version":1,"status":"passed","scenario":if stop_read {"stop_read"} else {"cancel"},
        "run_id":accepted.id,"process_attempt_id":attempt.id,"execution_receipt_sha256":canonical_hash(&receipt),
        "business_state":settled.run.state,"process_state":settled.process_attempt.state,
        "result_version_id":result_version_id,"manifest_sha256":manifest_sha256}),
    )?;
    if stop_read {
        println!(
            "controller_stop_read=verified; business_state=interrupted; original_attempts=1; local_ready=true; implicit_adoption=false"
        );
    } else {
        println!(
            "controller_cancel=verified; actual_started_guest=true; actual_store_cancel=true; original_signed_completion=true; business_state=canceled; original_attempts=1; replay_without_mutation=true; implicit_adoption=false"
        );
    }
    Ok(())
}
