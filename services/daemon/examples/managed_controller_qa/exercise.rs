//! Real confirmed Owner plan -> worker -> production TLS client -> local result.
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use cowork_broker::{FileBroker, FolderConfig};
use cowork_contracts::*;
use cowork_domain::{
    canonical_hash,
    environments::{EnvironmentDefinition, EnvironmentQualification},
    hash,
};
use cowork_runner_protocol::{Binding, ExecutionState};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use cowork_workflow_contracts::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
#[path = "cancel.rs"]
mod cancel;
#[path = "delivery.rs"]
mod delivery;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControllerContext {
    format_version: u32,
    database_url: String,
    project_id: String,
    order_id: String,
    environment_id: String,
    version_id: String,
    definition: EnvironmentDefinition,
    identity: ExecutionRunnerIdentity,
    target: ExecutionTargetRef,
    broker_directory: PathBuf,
    original_directory: PathBuf,
    folder_id: String,
    control_key_file: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Handoff {
    format_version: u32,
    controller_configuration_file: PathBuf,
    binding: Binding,
    target: ExecutionTargetRef,
    environment: ExecutionEnvironmentRef,
    definition: EnvironmentDefinition,
    qualification: EnvironmentQualification,
    host_sentinel: PathBuf,
    expected_input_utf8: String,
    program: String,
    mode: String,
    input_file_name: String,
    output_file_name: String,
    #[serde(default)]
    delivery: bool,
}

fn read_private<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    ensure!(path.is_absolute(), "Absolute private fixture path required");
    for ancestor in path.ancestors() {
        ensure!(
            !std::fs::symlink_metadata(ancestor)?
                .file_type()
                .is_symlink(),
            "Fixture symlink rejected"
        );
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let m = file.metadata()?;
    ensure!(
        m.is_file() && m.len() > 0 && m.len() <= 2 * 1024 * 1024,
        "Bounded private fixture file required"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            m.uid() == 0 && m.nlink() == 1 && m.mode() & 0o077 == 0,
            "Fixture authority must be private"
        );
    }
    let mut bytes = vec![];
    file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == m.len(),
        "Fixture file changed while reading"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Default)]
struct ModelFixture {
    arguments: Mutex<Option<Value>>,
    requests: AtomicUsize,
}

fn write_private_diagnostic(root: &Path, name: &str, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "Private controller diagnostic exceeded its bound"
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options.open(root.join(name))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    File::open(root)?.sync_all()?;
    Ok(())
}
async fn propose(
    State(state): State<Arc<ModelFixture>>,
    Json(request): Json<Value>,
) -> impl IntoResponse {
    let tools = request["tools"].as_array().is_some_and(|tools| {
        tools
            .iter()
            .any(|tool| tool["function"]["name"] == "execute_program")
    });
    if request["model"] != "controller-qa" || !tools {
        return (
            StatusCode::BAD_REQUEST,
            String::from("unexpected model request"),
        );
    }
    let sequence = state.requests.fetch_add(1, Ordering::SeqCst);
    let message = if sequence == 0 {
        let args = state.arguments.lock().unwrap().clone();
        let Some(args) = args else {
            return (StatusCode::CONFLICT, String::from("proposal not prepared"));
        };
        json!({"role":"assistant","content":"","tool_calls":[{"id":"actual-native-probe","function":{"name":"execute_program","arguments":args}}]})
    } else if sequence == 1
        && request["messages"]
            .as_array()
            .is_some_and(|messages| messages.iter().any(|message| message["role"] == "tool"))
    {
        json!({"role":"assistant","content":"The declared native architecture probe finished."})
    } else {
        return (
            StatusCode::CONFLICT,
            String::from("unexpected model replay"),
        );
    };
    (
        StatusCode::OK,
        format!(
            "{}\n",
            json!({"model":"controller-qa","message":message,"done":true,"prompt_eval_count":32,"eval_count":16})
        ),
    )
}

pub async fn run(root: &Path, handoff_path: &Path) -> Result<()> {
    ensure!(
        root.is_absolute() && root.file_name().is_some_and(|name| name == "controller"),
        "Explicit controller fixture root required"
    );
    ensure!(
        handoff_path == root.join("managed-service-handoff.json"),
        "Unexpected handoff path"
    );
    let mut context: ControllerContext = read_private(&root.join("controller-context.json"))?;
    let handoff: Handoff = read_private(handoff_path)?;
    ensure!(
        context.format_version == 1 && handoff.format_version == 1,
        "Unknown fixture format"
    );
    ensure!(
        context.database_url
            == format!(
                "sqlite:{}?mode=rw",
                root.join("controller.sqlite").display()
            )
            && context.broker_directory == root.join("broker")
            && context.original_directory == root.join("originals")
            && context.control_key_file == root.join("controller-control.pk8")
            && context.folder_id == "source",
        "Unexpected controller paths"
    );
    ensure!(
        handoff.controller_configuration_file == root.join("daemon-runner.json")
            && std::env::var_os("COWORK_PROCESS_RUNNER_CONFIG").as_deref()
                == Some(handoff.controller_configuration_file.as_os_str()),
        "Actual daemon configuration must be selected by the parent"
    );
    ensure!(
        handoff.definition == context.definition
            && handoff.target == context.target
            && handoff.environment.environment_id == context.environment_id
            && handoff.environment.version_id == context.version_id
            && handoff.environment.definition_sha256 == canonical_hash(&context.definition)
            && handoff.environment.qualification_sha256 == canonical_hash(&handoff.qualification),
        "Native handoff does not bind the original Store intent"
    );
    ensure!(
        handoff.program == "probe"
            && matches!(
                handoff.mode.as_str(),
                "owner_copy" | "owner_block" | "owner_stop_read"
            )
            && handoff.input_file_name == "value.txt"
            && handoff.output_file_name == "result.txt"
            && handoff.expected_input_utf8 == "approved HTTPS controller snapshot bytes\n"
            && handoff.host_sentinel.is_absolute(),
        "Unexpected declared fixture task"
    );
    ensure!(
        !handoff.delivery || handoff.mode == "owner_copy",
        "Delivery requires copy scenario"
    );
    let configured = crate::process_runner_config::configured()?
        .context("Actual daemon HTTPS configuration missing")?;
    ensure!(
        configured.execution_identity() == context.identity,
        "Daemon identity changed after prepare"
    );
    ensure!(
        handoff.binding.runner_id == context.identity.runner_id
            && handoff.binding.runner_public_key == context.identity.runner_public_key,
        "Native runner identity changed"
    );
    let source_before = std::fs::read(context.original_directory.join("value.txt"))?;
    ensure!(
        source_before == handoff.expected_input_utf8.as_bytes(),
        "Original fixture input changed"
    );
    let store = Store::connect(&context.database_url).await?;
    ensure!(
        store.runs(&context.order_id).await?.is_empty(),
        "Exercise requires its untouched original order"
    );
    let broker = FileBroker::open(
        &context.broker_directory,
        vec![FolderConfig {
            id: context.folder_id.clone(),
            project_id: context.project_id.clone(),
            name: "Source".into(),
            path: context.original_directory.clone(),
            writable: true,
        }],
    )?;
    let folders = broker.grants(&context.project_id)?;
    let model = Arc::new(ModelFixture::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let stop_model = CancellationToken::new();
    let model_stop = stop_model.clone();
    let app = Router::new()
        .route(
            "/api/tags",
            get(|| async { Json(json!({"models":[{"name":"controller-qa"}]})) }),
        )
        .route("/api/chat", post(propose))
        .with_state(model.clone());
    let model_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(model_stop.cancelled_owned())
            .await
    });
    let profile: ProviderProfile = serde_json::from_value(
        json!({"id":"controller-qa","name":"Declared local QA proposal fixture","kind":"ollama","endpoint":format!("http://{address}"),"model":"controller-qa","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true}),
    )?;
    store.insert_provider(&profile).await?;
    let runtime = Runtime::new(
        store.clone(),
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        false,
    )?
    .with_file_broker(broker)
    .with_registered_process_runner(configured)
    .await?;
    let staged = store
        .environment(&context.project_id, &context.environment_id)
        .await?;
    let qualified = runtime
        .qualify_process_environment(
            &context.project_id,
            &context.environment_id,
            &context.version_id,
            staged.revision,
            "actual-native-install",
        )
        .await?;
    runtime
        .activate_process_environment(
            &context.project_id,
            &context.environment_id,
            &context.version_id,
            qualified.revision,
            "actual-native-observe",
        )
        .await?;
    let mut jobs = Vec::new();
    let mut retained = Vec::new();
    let count = if handoff.delivery { 2 } else { 1 };
    for index in 1..=count {
        if index > 1 {
            context.order_id = store
                .create_order(CreateOrder {
                    project_id: context.project_id.clone(),
                    title: format!("Confirmed native delivery {index}"),
                })
                .await?
                .id;
            model.requests.store(0, Ordering::SeqCst);
        }
        runtime.workspace(&context.order_id).await?;
        let order = store.order(&context.order_id).await?;
        let input = runtime
            .prepare_execution_input(
                &context.order_id,
                &PrepareExecutionInput {
                    idempotency_key: "actual-controller-capture".into(),
                    base_revision: order.revision,
                    folder_ids: vec![context.folder_id.clone()],
                },
            )
            .await?;
        let alias = input
            .snapshot
            .sources
            .iter()
            .find(|source| source.name == "Source")
            .context("Captured Source alias missing")?
            .alias
            .clone();
        ensure!(
            !alias.contains('/') && !alias.is_empty(),
            "Invalid captured namespace"
        );
        *model.arguments.lock().unwrap() = Some(
            json!({"program":"probe","arguments":[handoff.mode,format!("{alias}/value.txt"),format!("{alias}/result.txt"),handoff.host_sentinel],"timeout_seconds":20}),
        );
        let request = serde_json::from_value(
            json!({"order_id":context.order_id,"base_revision":order.revision,"idempotency_key":"actual-controller-run","prompt":"Run the declared copy probe once in the confirmed isolated working copy.","provider_id":"controller-qa","provider_revision":1,"access_mode":"autonomous","budgets":Budgets::default(),"folders":folders,"environment":handoff.environment}),
        )?;
        let prepared = runtime
            .prepare_execution_plan_v2(&PrepareOwnerExecutionPlanV2 {
                target: OwnerExecutionTargetV2::RegisteredRunner {},
                request: RunRequest::Single(request),
                input: ExecutionInputRef {
                    snapshot_id: input.snapshot.id,
                    sha256: input.sha256,
                    manifest_sha256: input.snapshot.manifest.sha256,
                },
            })
            .await?;
        let confirmation = CommitExecutionPlan {
            sha256: prepared.sha256.clone(),
        };
        let accepted = runtime
            .commit_execution_plan_v2(&context.order_id, &prepared.plan.id, &confirmation)
            .await?;
        let stop_worker = CancellationToken::new();
        let worker = tokio::spawn(runtime.clone().serve(1, stop_worker.clone()));
        if matches!(handoff.mode.as_str(), "owner_block" | "owner_stop_read") {
            let result = cancel::run(
                root,
                &runtime,
                &store,
                &context,
                &handoff,
                &accepted,
                &prepared,
                &confirmation,
                &source_before,
                &model,
            )
            .await;
            stop_worker.cancel();
            tokio::time::timeout(Duration::from_secs(5), worker).await??;
            stop_model.cancel();
            tokio::time::timeout(Duration::from_secs(5), model_task).await???;
            if let Err(error) = &result {
                write_private_diagnostic(
                    root,
                    "controller-failure.json",
                    &json!({"format_version":1,
                "stage":"actual_cancel_reconciliation","run_id":accepted.id,
                "run":store.stored_run(&accepted.id).await?,"events":store.events(&accepted.id,0).await?,
                "process_attempts":store.process_attempts(&accepted.id).await?,"error":error.to_string()}),
                )?;
            }
            store.close().await;
            return result;
        }
        let deadline = Instant::now() + Duration::from_secs(100);
        let outcome = loop {
            let detail = store.detail(&accepted.id).await?;
            if matches!(
                detail.run.state,
                RunState::Completed
                    | RunState::Failed
                    | RunState::Canceled
                    | RunState::Expired
                    | RunState::Interrupted
            ) {
                break detail;
            }
            ensure!(
                Instant::now() < deadline,
                "Original worker did not settle in the bounded fixture"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        stop_worker.cancel();
        tokio::time::timeout(Duration::from_secs(5), worker).await??;
        if outcome.run.state != RunState::Completed {
            // The fixture has no model/MCP credentials. This sidecar stays private
            // and is never projected into Owner DTOs or the public progress report.
            let stored = store.stored_run(&accepted.id).await?;
            write_private_diagnostic(
                root,
                "controller-failure.json",
                &json!({
                    "format_version":1,"stage":"actual_worker","run_id":accepted.id,
                    "state":outcome.run.state,"error_code":outcome.run.error_code,
                    "run_revision":outcome.run.revision,"steps":outcome.run.steps,
                    "process_attempts":outcome.process_attempts,
                    "events":store.events(&accepted.id,0).await?,
                    "in_flight":stored.checkpoint.in_flight,
                    "pending_tools":stored.checkpoint.pending_tools,
                    "messages":stored.checkpoint.messages,
                    "completion_pending":stored.checkpoint.completion_pending
                }),
            )?;
            anyhow::bail!(
                "Actual worker failed: {:?}, {:?}",
                outcome.run.state,
                outcome.run.error_code
            );
        }
        ensure!(
            model.requests.load(Ordering::SeqCst) == 2 && outcome.process_attempts.len() == 1,
            "Expected one real process attempt and two declared model responses"
        );
        let attempt = &outcome.process_attempts[0].id;
        let receipt = store
            .managed_completion_receipt(&accepted.id, attempt)
            .await?
            .context("Original signed completion not retained")?;
        receipt.verify("receipt", &handoff.binding.runner_public_key)?;
        ensure!(
            matches!(&receipt.payload.state, ExecutionState::Completed{result} if result.termination == ProcessTermination::Exited && result.exit_code == Some(0) && result.output_complete),
            "Native completion did not prove success"
        );
        let manifest = runtime
            .managed_result_manifest(&context.order_id, &accepted.id, attempt)
            .await?;
        let version = runtime
            .retain_managed_result_version(&context.order_id, &accepted.id, attempt)
            .await?;
        let output = manifest
            .manifest
            .files
            .iter()
            .find(|file| file.path == format!("{alias}/result.txt"))
            .context("Native derived result missing")?;
        let mut output_bytes = vec![];
        for chunk in &output.chunks {
            let bytes = runtime
                .managed_result_version_chunk(
                    &context.order_id,
                    &accepted.id,
                    attempt,
                    &version,
                    ReadWorkspaceTransferChunk {
                        path: output.path.clone(),
                        offset: chunk.offset,
                        expected_sha256: chunk.sha256.clone(),
                    },
                )
                .await?;
            ensure!(
                bytes.len() as u64 == chunk.size && hash(&bytes) == chunk.sha256,
                "Local declared result chunk mismatch"
            );
            output_bytes.extend_from_slice(&bytes);
        }
        ensure!(
            output_bytes == handoff.expected_input_utf8.as_bytes()
                && hash(&output_bytes) == output.sha256,
            "Local native result bytes differ"
        );
        ensure!(
            runtime
                .retain_managed_result_version(&context.order_id, &accepted.id, attempt)
                .await?
                == version,
            "Original result replay changed version"
        );
        ensure!(
            runtime
                .commit_execution_plan_v2(&context.order_id, &prepared.plan.id, &confirmation)
                .await?
                .id
                == accepted.id
                && store.runs(&context.order_id).await?.len() == 1
                && store.process_attempts(&accepted.id).await?.len() == 1,
            "Original plan replay redispatched"
        );
        ensure!(
            std::fs::read(context.original_directory.join("value.txt"))? == source_before
                && !context.original_directory.join("result.txt").exists(),
            "Original source changed"
        );
        ensure!(
            runtime
                .run_copy_text(
                    &context.order_id,
                    &accepted.id,
                    FileLocation {
                        folder_id: context.folder_id.clone(),
                        path: "value.txt".into()
                    }
                )
                .await?
                .content
                .as_bytes()
                == source_before,
            "Confirmed local copy changed"
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
            "Remote output was implicitly adopted into the local copy"
        );
        ensure!(
            manifest.attestation.payload.execution_receipt_sha256 == canonical_hash(&receipt),
            "Result did not bind original completion"
        );
        if handoff.delivery {
            delivery::confirm(delivery::Case {
                root,
                index,
                runtime: &runtime,
                context: &context,
                run: &accepted.id,
                attempt,
                receipt: &receipt,
                result: &manifest,
                version: &version,
            })
            .await?;
            retained.push(delivery::LocalResult {
                order: context.order_id.clone(),
                run: accepted.id.clone(),
                attempt: attempt.clone(),
                version: version.clone(),
                manifest: manifest.manifest.clone(),
                expected: handoff.expected_input_utf8.as_bytes().to_vec(),
                original_directory: context.original_directory.clone(),
                folder_id: context.folder_id.clone(),
            });
            // Recheck every retained pack, source and confirmed copy after each native cleanup.
            for result in &retained {
                result.verify(&runtime).await?;
            }
        }
        jobs.push(
            json!({"index":index,"order_id":context.order_id,"run_id":accepted.id,
        "process_attempt_id":attempt,"result_version_id":version,
        "manifest_sha256":manifest.manifest.sha256,
        "execution_receipt_sha256":canonical_hash(&receipt)}),
        );
    }
    stop_model.cancel();
    tokio::time::timeout(Duration::from_secs(5), model_task).await???;
    let bytes = serde_json::to_vec(&if handoff.delivery {
        json!({"format_version":1,"status":"passed","delivery":true,"jobs":jobs,"both_local_results_readable_after_gc":true,"originals_and_copies_unchanged_after_each_gc":true})
    } else {
        let mut value = jobs.remove(0);
        value["format_version"] = json!(1);
        value["status"] = json!("passed");
        value
    })?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut result = options.open(root.join("controller-result.json"))?;
    result.write_all(&bytes)?;
    result.sync_all()?;
    File::open(root)?.sync_all()?;
    store.close().await;
    println!(
        "controller_exercise=verified; actual_worker=true; model=declared_fixture; jobs={count}; local_result_ready=true; implicit_adoption=false"
    );
    Ok(())
}
