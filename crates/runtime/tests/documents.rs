//! Built-in document tools: automatic grants, access-mode approvals and create-only publishing.
//! The engine flow needs a Python with the pinned document wheels:
//! COWORK_TEST_DOCUMENT_PYTHON=<path to python> cargo test -p cowork-runtime --test documents
use base64::Engine;
use cowork_broker::{FileBroker, FolderConfig};
use cowork_contracts::*;
use cowork_domain::{Error, StoredRun};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::{Change, Store};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

const PIXEL: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

struct Fixture {
    _dir: tempfile::TempDir,
    folder: std::path::PathBuf,
    runtime: Runtime,
    toolbox: Arc<cowork_toolbox::Toolbox>,
    input: CreateRun,
}

async fn setup(python: Option<std::path::PathBuf>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("work");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join("pixel.png"),
        base64::engine::general_purpose::STANDARD
            .decode(PIXEL)
            .unwrap(),
    )
    .unwrap();
    let store = Store::sqlite(&dir.path().join("runtime.db")).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Documents".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Document order".into(),
        })
        .await
        .unwrap();
    let broker = FileBroker::open(
        &dir.path().join("broker"),
        vec![FolderConfig {
            id: "root".into(),
            project_id: project.id.clone(),
            name: "Reports".into(),
            path: folder.clone(),
            writable: true,
        }],
    )
    .unwrap();
    let folders = broker.grants(&project.id).unwrap();
    let mut programs = BTreeMap::new();
    if let Some(python) = python {
        programs.insert("python".to_owned(), python);
    }
    let environment = ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"]
        .into_iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_owned(), value))
        })
        .collect();
    let toolbox = cowork_toolbox::Toolbox::open(cowork_toolbox::Config {
        root: dir.path().join("toolbox"),
        programs,
        environment,
        github_token: None,
    })
    .unwrap();
    let profile: ProviderProfile = serde_json::from_value(json!({
        "id":"test","name":"Fixture","kind":"test","endpoint":"","model":"test","revision":1,
        "context_tokens":32768,"credential_required":false,"credential_ready":true
    }))
    .unwrap();
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        true,
    )
    .unwrap()
    .with_file_broker(broker)
    .with_toolbox(toolbox.clone());
    let input = CreateRun {
        agent_config: None,
        model_binding: None,
        output: None,
        native_tools: vec![],
        environment: None,
        mcp_tools: vec![],
        budgets: Budgets::default(),
        order_id: order.id,
        base_revision: 1,
        idempotency_key: "document-start".into(),
        prompt: "Edit the picture".into(),
        provider_id: "test".into(),
        provider_revision: 1,
        access_mode: AccessMode::ConfirmMutations,
        folders,
    };
    Fixture {
        _dir: dir,
        folder,
        runtime,
        toolbox,
        input,
    }
}

async fn seed(f: &Fixture, input: &CreateRun, tool: &ToolCall) -> RunRecord {
    // A preflight may register the managed workspace, which advances the order revision.
    let mut input = input.clone();
    input.base_revision = f
        .runtime
        .store
        .order(&input.order_id)
        .await
        .unwrap()
        .revision;
    let run = f.runtime.create_run(&input).await.unwrap();
    f.runtime
        .store
        .mutate(&run.id, None, |r| {
            if !matches!(r.record.state, RunState::Queued | RunState::WaitingForExecutor) || r.lease.is_some() {
                return Err(Error::Conflict);
            }
            r.checkpoint.pending_tools = vec![tool.clone()];
            r.checkpoint.messages.push(json!({"role":"assistant","content":"Fixture tool request","tool_calls":[{"id":tool.id,"type":"function","function":{"name":tool.name,"arguments":tool.arguments.to_string()}}]}));
            Ok(Change::event("fixture_model_reply", json!({})))
        })
        .await
        .unwrap();
    run
}

async fn until<F: Fn(&StoredRun) -> bool>(f: &Fixture, id: &str, check: F) -> StoredRun {
    for _ in 0..1500 {
        let run = f.runtime.store.stored_run(id).await.unwrap();
        if check(&run) {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let run = f.runtime.store.stored_run(id).await.unwrap();
    panic!(
        "condition not reached: {:?} events={:?}",
        run.record,
        f.runtime.store.events(id, 0).await.unwrap()
    );
}

fn tool_result(run: &StoredRun, id: &str) -> serde_json::Value {
    let message = run
        .checkpoint
        .messages
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
        .expect("tool result");
    serde_json::from_str(message["content"].as_str().unwrap()).unwrap()
}

fn engine_python() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("COWORK_TEST_DOCUMENT_PYTHON") {
        let path = std::path::PathBuf::from(path);
        assert!(
            path.is_file(),
            "COWORK_TEST_DOCUMENT_PYTHON must name an existing interpreter"
        );
        return Some(path);
    }
    assert!(
        std::env::var_os("COWORK_REQUIRE_DOCUMENT_TESTS").is_none(),
        "COWORK_TEST_DOCUMENT_PYTHON is required for the document qualification job"
    );
    None
}

async fn ready(toolbox: &cowork_toolbox::Toolbox) -> bool {
    for _ in 0..300 {
        if toolbox.availability().documents_available {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn workspace_lookup_waits_for_short_contention_and_bounds_a_persistent_lock() {
    let f = setup(None).await;
    let order = &f.input.order_id;
    let expected = f.runtime.workspace(order).await.unwrap();
    let path = f
        ._dir
        .path()
        .join("broker/locks")
        .join(format!("workspace-{order}"));
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    lock.lock().unwrap();
    let release = async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        drop(lock);
    };
    let (observed, ()) = tokio::join!(f.runtime.workspace(order), release);
    assert_eq!(observed.unwrap(), expected);

    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    lock.lock().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(3), f.runtime.workspace(order))
        .await
        .expect("workspace contention must have a finite bound");
    assert!(matches!(result, Err(Error::WorkspaceBusy)));
    drop(lock);
    assert_eq!(f.runtime.workspace(order).await.unwrap(), expected);
}

#[tokio::test]
async fn document_tools_are_only_granted_when_the_engine_is_ready() {
    let f = setup(None).await;
    let flight = f.runtime.preflight(&f.input).await.unwrap();
    assert!(
        !flight
            .allowed_tools
            .iter()
            .any(|t| cowork_domain::document_tool(t)),
        "{:?}",
        flight.allowed_tools
    );
    // A saved agent filter naming document tools narrows instead of failing while they are missing.
    let mut filtered = f.input.clone();
    filtered.agent_config = Some(EffectiveAgentConfig {
        tool_filter: Some(vec![
            "list_files".into(),
            "document_read".into(),
            "document_write".into(),
        ]),
        ..Default::default()
    });
    let flight = f.runtime.preflight(&filtered).await.unwrap();
    assert_eq!(flight.allowed_tools, vec!["list_files".to_owned()]);
}

#[tokio::test]
async fn document_writes_follow_the_access_mode_and_never_overwrite() {
    document_flow(&[AccessMode::ConfirmMutations, AccessMode::Autonomous]).await;
}

#[tokio::test]
async fn read_only_runs_deny_document_writes() {
    document_flow(&[AccessMode::ReadOnly]).await;
}

#[tokio::test]
async fn read_only_runs_can_read_documents_in_a_managed_workspace() {
    let Some(python) = engine_python() else {
        return;
    };
    let f = setup(Some(python)).await;
    assert!(ready(&f.toolbox).await);
    let workspace = f.runtime.workspace(&f.input.order_id).await.unwrap();
    let mut input = f.input.clone();
    input.access_mode = AccessMode::ReadOnly;
    input.idempotency_key = "document-read-only-read".into();
    let tool = ToolCall {
        id: "read-only-1".into(),
        name: "document_read".into(),
        arguments: json!({"operation":"metadata","folder_id":"root","path":"pixel.png"}),
    };
    let run = seed(&f, &input, &tool).await;
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    let done = until(&f, &run.id, |r| r.record.state == RunState::Completed).await;
    stop.cancel();
    worker.await.unwrap();
    let result = tool_result(&done, &tool.id);
    assert_eq!(result["result"]["width"], 1, "{result}");
    assert!(result.get("error").is_none(), "{result}");
    let grant = done
        .record
        .spec
        .folders
        .iter()
        .find(|g| g.id == workspace.folder.id)
        .unwrap();
    assert!(!grant.writable);
    assert!(
        f.runtime
            .store
            .file_operations(&run.id)
            .await
            .unwrap()
            .is_empty()
    );
}

fn document_request(
    base_revision: i64,
    key: &str,
    mode: AccessMode,
    tool: ToolCall,
) -> DocumentActionRequest {
    DocumentActionRequest {
        version: 2,
        base_revision,
        idempotency_key: key.into(),
        access_mode: mode,
        tool,
    }
}

#[tokio::test]
async fn direct_document_actions_are_durable_model_free_and_bound_to_reviewed_sources() {
    let Some(python) = engine_python() else {
        return;
    };
    let f = setup(Some(python)).await;
    assert!(ready(&f.toolbox).await);
    let order = &f.input.order_id;
    let revision = f.runtime.store.order(order).await.unwrap().revision;
    let request = document_request(
        revision,
        "direct-document",
        AccessMode::Autonomous,
        ToolCall {
            id: "client".into(),
            name: "document_write".into(),
            arguments: json!({"operation":"create","output":{"path":"direct.docx"},"spec":{"format":"docx","markdown":"# Direkt\n\nOhne Modell."}}),
        },
    );
    let prepared = f
        .runtime
        .prepare_document_action(order, &request)
        .await
        .unwrap();
    assert!(!prepared.requires_approval);
    assert_eq!(
        f.runtime
            .prepare_document_action(order, &request)
            .await
            .unwrap()
            .id,
        prepared.id
    );
    let mut changed = request.clone();
    changed.tool.arguments["spec"]["markdown"] = json!("different");
    assert!(
        f.runtime
            .prepare_document_action(order, &changed)
            .await
            .is_err()
    );
    let run = f
        .runtime
        .commit_document_action(order, &prepared.id)
        .await
        .unwrap();
    assert!(run.spec.provider.is_none());
    assert_eq!(
        f.runtime
            .commit_document_action(order, &prepared.id)
            .await
            .unwrap()
            .id,
        run.id
    );
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    let done = until(&f, &run.id, |r| {
        matches!(r.record.state, RunState::Completed | RunState::Failed)
    })
    .await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(done.record.state, RunState::Completed, "{:?}", done.record);
    assert!(done.record.model_attempts.is_empty());
    let operation = f
        .runtime
        .store
        .file_operations(&run.id)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(operation.kind, FileOperationKind::Generate);
    assert_eq!(
        f.runtime
            .document_preparation(order, &prepared.id)
            .await
            .unwrap()
            .run_id,
        Some(run.id.clone())
    );
    // A lost commit ACK returns the original run even after the destination was published.
    assert_eq!(
        f.runtime
            .commit_document_action(order, &prepared.id)
            .await
            .unwrap()
            .id,
        run.id
    );
    let read = document_request(
        f.runtime.store.order(order).await.unwrap().revision,
        "direct-read",
        AccessMode::ReadOnly,
        ToolCall {
            id: "read".into(),
            name: "document_read".into(),
            arguments: json!({"operation":"text","folder_id":operation.folder_id,"path":operation.path}),
        },
    );
    let prepared = f
        .runtime
        .prepare_document_action(order, &read)
        .await
        .unwrap();
    assert_eq!(
        prepared.tool.arguments["expected_sha256"],
        json!(operation.after.as_ref().unwrap().sha256)
    );
    let run = f
        .runtime
        .commit_document_action(order, &prepared.id)
        .await
        .unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    let done = until(&f, &run.id, |r| {
        matches!(r.record.state, RunState::Completed | RunState::Failed)
    })
    .await;
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(done.record.state, RunState::Completed, "{:?}", done.record);
    assert!(
        tool_result(&done, &prepared.tool.id)["result"]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("Ohne Modell")
    );
    // Source hashes are bound during prepare, not chosen afresh when committing.
    let revision = f.runtime.store.order(order).await.unwrap().revision;
    let request = document_request(
        revision,
        "changed-source",
        AccessMode::ReadOnly,
        ToolCall {
            id: "read".into(),
            name: "document_read".into(),
            arguments: json!({"operation":"metadata","folder_id":"root","path":"pixel.png"}),
        },
    );
    let prepared = f
        .runtime
        .prepare_document_action(order, &request)
        .await
        .unwrap();
    std::fs::write(f.folder.join("pixel.png"), b"changed").unwrap();
    assert!(matches!(
        f.runtime.commit_document_action(order, &prepared.id).await,
        Err(Error::FileConflict)
    ));
    let foreign = f
        .runtime
        .store
        .create_order(CreateOrder {
            project_id: f.input.folders[0].project_id.clone(),
            title: "Other".into(),
        })
        .await
        .unwrap();
    assert!(
        f.runtime
            .document_preparation(&foreign.id, &prepared.id)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn document_action_refuses_a_changed_runtime_after_review() {
    let Some(python) = engine_python() else {
        return;
    };
    let f = setup(Some(python.clone())).await;
    assert!(ready(&f.toolbox).await);
    // Repairing the same configured paths must invalidate an earlier probe too.
    let reloaded = f.toolbox.reload_programs(BTreeMap::new()).unwrap();
    assert_eq!(reloaded.document_status, "probing");
    assert!(ready(&f.toolbox).await);
    let order = &f.input.order_id;
    let request = document_request(
        f.runtime.store.order(order).await.unwrap().revision,
        "runtime-changed",
        AccessMode::ReadOnly,
        ToolCall {
            id: "read".into(),
            name: "document_read".into(),
            arguments: json!({"operation":"metadata","folder_id":"root","path":"pixel.png"}),
        },
    );
    let prepared = f
        .runtime
        .prepare_document_action(order, &request)
        .await
        .unwrap();
    // Replacing executable selection after review must not silently run another engine.
    let replacement = f._dir.path().join(if cfg!(windows) {
        "replacement-python.exe"
    } else {
        "replacement-python"
    });
    std::fs::copy(&python, &replacement).unwrap();
    f.toolbox
        .reload_programs(BTreeMap::from([("python".into(), replacement)]))
        .unwrap();
    assert!(matches!(
        f.runtime.commit_document_action(order, &prepared.id).await,
        Err(Error::Conflict)
    ));
    f.toolbox
        .reload_programs(BTreeMap::from([("python".into(), python)]))
        .unwrap();
    assert!(ready(&f.toolbox).await);
    assert_eq!(
        f.toolbox.document_engine_revision(),
        prepared.engine_revision
    );
    assert!(
        f.runtime
            .commit_document_action(order, &prepared.id)
            .await
            .is_ok()
    );
}

async fn document_flow(modes: &[AccessMode]) {
    let Some(python) = engine_python() else {
        eprintln!("COWORK_TEST_DOCUMENT_PYTHON is not set; skipping the engine flow");
        return;
    };
    for mode in modes.iter().cloned() {
        let f = setup(Some(python.clone())).await;
        assert!(ready(&f.toolbox).await, "document engine probe failed");
        let mut input = f.input.clone();
        input.access_mode = mode.clone();
        input.idempotency_key = format!("document-{mode:?}");
        let flight = f.runtime.preflight(&input).await.unwrap();
        let read_only = mode == AccessMode::ReadOnly;
        assert!(flight.allowed_tools.contains(&"document_read".to_owned()));
        assert_eq!(
            flight.allowed_tools.contains(&"image_edit".to_owned()),
            !read_only
        );
        let edit = ToolCall {
            id: "edit-1".into(),
            name: "document_write".into(),
            arguments: json!({"operation":"create","output":{"folder_id":"root","path":"pixel-gray.docx"},"spec":{"format":"docx","blocks":[{"type":"heading","text":"Bericht","level":1},{"type":"paragraph","text":"Umlaute äöü und Кириллица"}]}}),
        };
        let run = seed(&f, &input, &edit).await;
        let stop = CancellationToken::new();
        let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
        if mode == AccessMode::ConfirmMutations {
            let waiting = until(&f, &run.id, |r| r.record.state == RunState::WaitingApproval).await;
            let approval = waiting
                .checkpoint
                .approvals
                .iter()
                .find(|a| a.decision.is_none())
                .unwrap()
                .clone();
            f.runtime
                .store
                .resolve_approval(
                    &run.id,
                    &approval.id,
                    ResolveApproval {
                        fingerprint: approval.fingerprint,
                        allow: true,
                    },
                )
                .await
                .unwrap();
        }
        let done = until(&f, &run.id, |r| {
            r.checkpoint
                .messages
                .iter()
                .any(|m| m["role"] == "tool" && m["tool_call_id"] == "edit-1")
        })
        .await;
        let result = tool_result(&done, "edit-1");
        if read_only {
            assert_eq!(result["error"], "policy_denied");
            assert!(!f.folder.join("pixel-gray.docx").exists());
        } else {
            assert_eq!(result["kind"], "generate", "{result}");
            assert!(f.folder.join("pixel-gray.docx").is_file());
            let operations = f.runtime.store.file_operations(&run.id).await.unwrap();
            assert_eq!(operations[0].kind, FileOperationKind::Generate);
            assert_eq!(
                operations[0].generated_from.as_ref().unwrap().tool,
                "document_write"
            );
        }
        // Writable folders stay reserved until the run is terminal.
        until(&f, &run.id, |r| r.record.state == RunState::Completed).await;
        stop.cancel();
        let _ = worker.await;
        if read_only {
            continue;
        }
        // The same output path again: refused before any approval is requested.
        let mut again = input.clone();
        again.idempotency_key = format!("document-again-{mode:?}");
        let before = std::fs::read(f.folder.join("pixel-gray.docx")).unwrap();
        let repeat = ToolCall {
            id: "edit-2".into(),
            ..edit.clone()
        };
        let run = seed(&f, &again, &repeat).await;
        let stop = CancellationToken::new();
        let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
        let done = until(&f, &run.id, |r| {
            r.checkpoint
                .messages
                .iter()
                .any(|m| m["role"] == "tool" && m["tool_call_id"] == "edit-2")
        })
        .await;
        assert!(
            done.checkpoint.approvals.is_empty(),
            "no approval for a call that cannot succeed"
        );
        assert!(
            tool_result(&done, "edit-2")["message"]
                .as_str()
                .unwrap()
                .contains("never overwritten")
        );
        assert_eq!(
            std::fs::read(f.folder.join("pixel-gray.docx")).unwrap(),
            before
        );
        stop.cancel();
        let _ = worker.await;
    }
}

async fn run_tool(
    f: &Fixture,
    key: &str,
    tool: ToolCall,
) -> (RunRecord, serde_json::Value, StoredRun) {
    let mut input = f.input.clone();
    input.access_mode = AccessMode::Autonomous;
    input.idempotency_key = key.into();
    let run = seed(f, &input, &tool).await;
    let stop = CancellationToken::new();
    let worker = tokio::spawn(f.runtime.clone().serve(1, stop.clone()));
    let done = until(f, &run.id, |r| r.record.state == RunState::Completed).await;
    stop.cancel();
    let _ = worker.await;
    let result = tool_result(&done, &tool.id);
    (run, result, done)
}

#[tokio::test]
async fn image_edits_pdfs_and_previews_flow_through_the_runtime() {
    let Some(python) = engine_python() else {
        eprintln!("COWORK_TEST_DOCUMENT_PYTHON is not set; skipping the engine flow");
        return;
    };
    let f = setup(Some(python)).await;
    assert!(ready(&f.toolbox).await, "document engine probe failed");
    // Image edit with a rendered preview: a new file plus a retained preview reference.
    let edit = ToolCall {
        id: "image-1".into(),
        name: "image_edit".into(),
        arguments: json!({"input":{"folder_id":"root","path":"pixel.png"},"output":{"folder_id":"root","path":"pixel-big.png"},
            "steps":[{"op":"resize","width":64,"height":64,"fit":"stretch"},{"op":"grayscale"}],"preview":true}),
    };
    let (run, result, done) = run_tool(&f, "image", edit).await;
    assert_eq!(result["kind"], "generate", "{result}");
    let image = std::fs::read(f.folder.join("pixel-big.png")).unwrap();
    assert!(image.starts_with(b"\x89PNG"));
    let message = done
        .checkpoint
        .messages
        .iter()
        .find(|m| m["tool_call_id"] == "image-1")
        .unwrap();
    let preview = message["cowork_images"][0]["sha256"]
        .as_str()
        .expect("preview reference")
        .to_owned();
    assert!(
        f.runtime
            .run_preview(&run.id, &preview)
            .await
            .unwrap()
            .starts_with(b"\x89PNG")
    );
    assert!(
        f.runtime
            .run_preview(&run.id, &"0".repeat(64))
            .await
            .is_err(),
        "only referenced previews are readable"
    );
    // create_pdf without LaTeX uses the ReportLab layout.
    let pdf = ToolCall {
        id: "pdf-1".into(),
        name: "document_write".into(),
        arguments: json!({"operation":"create_pdf","output":{"folder_id":"root","path":"bericht.pdf"},
            "spec":{"title":"Bericht","markdown":"# Bericht\n\nUmsatz **stieg** um 12 %.\n\n- Nord\n- Süd"}}),
    };
    let (_, result, _) = run_tool(&f, "pdf", pdf).await;
    assert_eq!(result["kind"], "generate", "{result}");
    assert!(
        std::fs::read(f.folder.join("bericht.pdf"))
            .unwrap()
            .starts_with(b"%PDF")
    );
    // Reading and previewing the new PDF.
    let read = ToolCall {
        id: "read-1".into(),
        name: "document_read".into(),
        arguments: json!({"operation":"text","folder_id":"root","path":"bericht.pdf"}),
    };
    let (_, result, _) = run_tool(&f, "read", read).await;
    assert!(
        result["result"]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("Umsatz"),
        "{result}"
    );
    let look = ToolCall {
        id: "look-1".into(),
        name: "document_read".into(),
        arguments: json!({"operation":"preview","folder_id":"root","path":"bericht.pdf","pages":[1]}),
    };
    let (_, result, done) = run_tool(&f, "look", look).await;
    assert_eq!(result["previews"]["count"], 1, "{result}");
    assert_eq!(
        result["previews"]["shown_to_model"], false,
        "the fixture model has no image input"
    );
    let message = done
        .checkpoint
        .messages
        .iter()
        .find(|m| m["tool_call_id"] == "look-1")
        .unwrap();
    assert_eq!(message["cowork_images"].as_array().map(Vec::len), Some(1));
}
