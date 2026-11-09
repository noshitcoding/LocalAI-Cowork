//! Actual native engine/COW effects with no configured model or model account.
use base64::Engine;
use cowork_contracts::*;
use cowork_domain::{hash, workflows::*};
use cowork_runtime::Runtime;
use cowork_storage::Store;
use cowork_workflow_contracts::{
    CommitExecutionPlan, OwnerExecutionTargetV2, PrepareOwnerExecutionPlanV2, RunRequest,
    WorkflowRunRequest,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
mod review_sources;
mod table_append;

fn python() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("COWORK_TEST_DOCUMENT_PYTHON")
            .expect("Explicit real document Python is required"),
    );
    assert!(path.is_file());
    path
}
fn definition(pdf: bool) -> WorkflowDefinition {
    let mut source: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-native-documents.json"
    ))
    .unwrap();
    if pdf {
        let step = source.documents.get_mut("write-report").unwrap();
        step.operation = WorkflowDocumentOperation::Create {
            format: WorkflowDocumentFormat::Pdf,
            path_input: "path".into(),
            content_input: "content".into(),
        };
        let config = source
            .dataflow
            .as_mut()
            .unwrap()
            .steps
            .get_mut("write-report")
            .unwrap();
        config.output = step.operation.output_type();
        config.inputs.get_mut("path").unwrap().value = WorkflowDataSource::Literal {
            value: json!("report.pdf"),
        };
    }
    source
}
struct Fixture {
    temp: tempfile::TempDir,
    runtime: Runtime,
    toolbox: Arc<cowork_toolbox::Toolbox>,
    project: Project,
    order: Order,
    workflow: String,
    version: String,
    source_sha: String,
    source_name: String,
    pdf: bool,
}
impl Fixture {
    async fn new(pdf: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&temp.path().join("native-documents.sqlite3"))
            .await
            .unwrap();
        let project = store
            .create_project(CreateProject {
                name: "No-model document workflow".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Native document report".into(),
            })
            .await
            .unwrap();
        let source = definition(pdf);
        assert!(source.roles.is_empty());
        let entry = store
            .create_workflow(&project.id, "native-document-source", &source)
            .await
            .unwrap();
        let toolbox = cowork_toolbox::Toolbox::open(cowork_toolbox::Config {
            root: temp.path().join("toolbox"),
            programs: BTreeMap::from([("python".into(), python())]),
            environment: ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"]
                .into_iter()
                .filter_map(|name| std::env::var(name).ok().map(|value| (name.into(), value)))
                .collect(),
            github_token: None,
        })
        .unwrap();
        let until = tokio::time::Instant::now() + Duration::from_secs(30);
        while !toolbox.availability().documents_available {
            assert!(
                tokio::time::Instant::now() < until,
                "actual document engine did not become ready"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let runtime = Runtime::new(store, vec![], false)
            .unwrap()
            .with_file_broker(
                cowork_broker::FileBroker::open(&temp.path().join("broker"), vec![]).unwrap(),
            )
            .with_toolbox(toolbox.clone());
        runtime.workspace(&order.id).await.unwrap();
        let data = b"# Bound report\n\nOriginal frozen source.\n";
        let source_path = temp
            .path()
            .join("broker/workspaces")
            .join(&order.id)
            .join("files/source.txt");
        std::fs::write(&source_path, data).unwrap();
        Self {
            temp,
            runtime,
            toolbox,
            project,
            order,
            workflow: entry.id,
            version: entry.versions[0].id.clone(),
            source_sha: hash(data),
            source_name: "source.txt".into(),
            pdf,
        }
    }
    fn original(&self) -> PathBuf {
        self.temp
            .path()
            .join("broker/workspaces")
            .join(&self.order.id)
            .join("files")
    }
    async fn request(&self, mode: AccessMode) -> PrepareOwnerExecutionPlanV2 {
        let workspace = self.runtime.workspace(&self.order.id).await.unwrap();
        let input = self
            .runtime
            .prepare_execution_input(
                &self.order.id,
                &PrepareExecutionInput {
                    idempotency_key: "native-input".into(),
                    base_revision: self.order.revision,
                    folder_ids: vec![workspace.folder.id],
                },
            )
            .await
            .unwrap();
        let request:WorkflowRunRequest = serde_json::from_value(json!({"kind":"workflow","order_id":self.order.id,"base_revision":self.order.revision,"idempotency_key":"native-start","prompt":"Process the selected frozen source with the declared native actions.","workflow_id":self.workflow,"version_id":self.version,"access_mode":mode,"budgets":definition(self.pdf).budgets,"inputs":{"source_path":self.source_name,"source_sha256":self.source_sha}})).unwrap();
        PrepareOwnerExecutionPlanV2 {
            target: OwnerExecutionTargetV2::DocumentEngine {},
            request: RunRequest::Workflow(request),
            input: ExecutionInputRef {
                snapshot_id: input.snapshot.id,
                sha256: input.sha256,
                manifest_sha256: input.snapshot.manifest.sha256,
            },
        }
    }
    async fn commit(&self, mode: AccessMode) -> RunRecord {
        let prepare = self.request(mode).await;
        let prepared = self
            .runtime
            .prepare_execution_plan_v2(&prepare)
            .await
            .unwrap();
        assert!(
            self.runtime
                .store
                .runs(&self.order.id)
                .await
                .unwrap()
                .is_empty()
        );
        self.runtime
            .commit_execution_plan_v2(
                &self.order.id,
                &prepared.plan.id,
                &CommitExecutionPlan {
                    sha256: prepared.sha256,
                },
            )
            .await
            .unwrap()
    }
    async fn wait(&self, run: &str, predicate: impl Fn(&RunDetail) -> bool) -> RunDetail {
        let until = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let detail = self.runtime.store.detail(run).await.unwrap();
            if predicate(&detail) {
                return detail;
            }
            assert!(
                !matches!(
                    detail.run.state,
                    RunState::Failed
                        | RunState::Interrupted
                        | RunState::Expired
                        | RunState::Canceled
                ),
                "unexpected terminal state: {:?}",
                detail.run
            );
            assert!(
                tokio::time::Instant::now() < until,
                "last run state: {:?}",
                detail.run
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    async fn completed(&self, run: &str) -> RunDetail {
        let result = self
            .wait(run, |detail| {
                matches!(
                    detail.run.state,
                    RunState::Completed
                        | RunState::Failed
                        | RunState::Interrupted
                        | RunState::Expired
                )
            })
            .await;
        assert_eq!(result.run.state, RunState::Completed, "{:?}", result.run);
        result
    }
    async fn approval(&self, run: &str) -> Approval {
        let until = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let view = self
                .runtime
                .store
                .workflow_run_view(&self.project.id, run)
                .await
                .unwrap();
            if let Some(item) = view.approvals.first() {
                assert_eq!(view.approvals.len(), 1);
                assert!(
                    item.role_id.is_none(),
                    "a native action must not invent a model role"
                );
                let detail = self.runtime.store.detail(run).await.unwrap();
                assert!(detail.file_operations.is_empty() && detail.run.model_attempts.is_empty());
                return item.approval.clone();
            }
            assert!(
                !matches!(
                    view.run.state,
                    RunState::Failed
                        | RunState::Interrupted
                        | RunState::Expired
                        | RunState::Canceled
                ),
                "unexpected terminal state: {:?}, code={:?}, nodes={:?}",
                view.run.state,
                view.run.error_code,
                view.nodes
            );
            assert!(
                tokio::time::Instant::now() < until,
                "native approval missing: state={:?}, nodes={:?}",
                view.run.state,
                view.nodes
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    async fn assert_native(&self, run: &str, nodes: usize) {
        let stored = self
            .runtime
            .store
            .stored_workflow(&self.project.id, run)
            .await
            .unwrap();
        assert!(stored.plan.profiles.is_empty() && stored.plan.roles.is_empty());
        assert_eq!(stored.progress.charged_tokens(), 0);
        assert!(
            stored
                .progress
                .attempts()
                .values()
                .all(|attempt| !matches!(attempt.operation, WorkflowOperation::Model { .. }))
        );
        assert_eq!(stored.agents.len(), nodes);
        for agent in stored.agents.values() {
            assert_eq!(agent.turns.len(), 1);
            let turn = &agent.turns[0];
            assert!(
                turn.deterministic
                    && turn.context.spec.provider.is_none()
                    && turn.context.spec.model_binding.is_none()
            );
            assert_eq!(turn.tool_results.len(), 1);
            assert!(turn.tool_results[0].succeeded);
        }
    }
    async fn preview_source(&mut self, name: &str, data: &[u8], pages: Vec<u32>) {
        self.read_source(
            name,
            data,
            WorkflowDocumentOperation::Preview {
                path_input: "path".into(),
                sha256_input: "sha".into(),
                pages,
            },
        )
        .await;
    }
    async fn read_source(&mut self, name: &str, data: &[u8], operation: WorkflowDocumentOperation) {
        let mut source = definition(false);
        source.documents.remove("write-report");
        let step = source.documents.get_mut("read-source").unwrap();
        step.operation = operation;
        let flow = source.dataflow.as_mut().unwrap();
        flow.steps.remove("write-report");
        flow.steps.get_mut("read-source").unwrap().output = step.operation.output_type();
        let entry = self
            .runtime
            .store
            .create_workflow(&self.project.id, "native-preview-source", &source)
            .await
            .unwrap();
        self.workflow = entry.id;
        self.version = entry.versions[0].id.clone();
        std::fs::write(self.original().join(name), data).unwrap();
        self.source_sha = hash(data);
        self.source_name = name.into();
    }
    async fn ocr_source(&mut self, models: bool, offset: u32, max_words: u16) {
        if models {
            let pack = PathBuf::from(
                std::env::var_os("COWORK_TEST_OCR_MODELS").expect("Exact OCR pack required"),
            );
            let root = self.temp.path().join("ocr-runtime");
            std::fs::create_dir_all(root.join("ocr/tessdata")).unwrap();
            for language in ["deu", "eng"] {
                std::fs::copy(
                    pack.join(format!("{language}.traineddata")),
                    root.join(format!("ocr/tessdata/{language}.traineddata")),
                )
                .unwrap();
            }
            self.toolbox.configure_document_runtime(root).unwrap();
            let until = tokio::time::Instant::now() + Duration::from_secs(30);
            while !self.toolbox.availability().ocr_available {
                assert!(
                    tokio::time::Instant::now() < until,
                    "OCR runtime not ready: {:?}",
                    self.toolbox.availability()
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        self.read_source(
            "invoice.pdf",
            include_bytes!("../../../tests/fixtures/ocr-invoice-scan.pdf"),
            WorkflowDocumentOperation::Ocr {
                path_input: "path".into(),
                sha256_input: "sha".into(),
                languages: vec!["deu".into(), "eng".into()],
                pages: vec![],
                dpi: 200,
                rotation: 0,
                offset,
                max_words,
            },
        )
        .await;
    }
    fn report(&self) -> PathBuf {
        fn find(directory: &Path, name: &str, found: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    find(&path, name, found);
                } else if path.file_name().and_then(|s| s.to_str()) == Some(name) {
                    found.push(path);
                }
            }
        }
        let name = if self.pdf {
            "report.pdf"
        } else {
            "report.docx"
        };
        let mut found = vec![];
        find(&self.temp.path().join("broker"), name, &mut found);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(!self.original().join(name).exists());
        found.remove(0)
    }
}
struct Worker {
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Worker {
    fn start(runtime: Runtime) -> Self {
        let stop = CancellationToken::new();
        let owned = stop.clone();
        let task = tokio::spawn(runtime.serve(1, owned));
        Self {
            stop,
            task: Some(task),
        }
    }
    async fn close(mut self) {
        self.stop.cancel();
        self.task.take().unwrap().await.unwrap();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn native_docx_reads_the_confirmed_copy_and_writes_a_report_without_any_model_account() {
    let f = Fixture::new(false).await;
    let run = f.commit(AccessMode::Autonomous).await;
    std::fs::write(f.original().join("source.txt"), b"UNBOUND CHANGED ORIGINAL").unwrap();
    let worker = Worker::start(f.runtime.clone());
    let done = f.completed(&run.id).await;
    assert!(done.run.model_attempts.is_empty());
    assert_eq!(done.file_operations.len(), 1);
    f.assert_native(&run.id, 2).await;
    let report = f.report();
    let output=std::process::Command::new(python()).args(["-I","-c","import sys,zipfile,xml.etree.ElementTree as E; z=zipfile.ZipFile(sys.argv[1]); s=''.join(E.fromstring(z.read('word/document.xml')).itertext()); assert 'Original frozen source.' in s and 'UNBOUND CHANGED ORIGINAL' not in s"]).arg(&report).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    worker.close().await;
}

#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn native_pdf_waits_for_one_original_approval_without_model_context_or_a_second_file() {
    let f = Fixture::new(true).await;
    let run = f.commit(AccessMode::ConfirmMutations).await;
    let worker = Worker::start(f.runtime.clone());
    let approval = f.approval(&run.id).await;
    f.runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint.clone(),
                allow: true,
            },
        )
        .await
        .unwrap();
    let done = f.completed(&run.id).await;
    assert_eq!(done.approvals.len(), 1);
    assert_eq!(done.file_operations.len(), 1);
    f.assert_native(&run.id, 2).await;
    let output=std::process::Command::new(python()).args(["-I","-c","import sys; from pypdf import PdfReader; assert 'Original frozen source.' in ''.join(p.extract_text() or '' for p in PdfReader(sys.argv[1]).pages)"]).arg(f.report()).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    worker.close().await;
}

#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn an_engine_change_after_native_review_never_publishes_the_old_approved_action() {
    let f = Fixture::new(false).await;
    let run = f.commit(AccessMode::ConfirmMutations).await;
    let worker = Worker::start(f.runtime.clone());
    let approval = f.approval(&run.id).await;
    f.toolbox
        .configure_document_runtime(f.temp.path().join("changed-native-engine-cache"))
        .unwrap();
    f.runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint.clone(),
                allow: true,
            },
        )
        .await
        .unwrap();
    let done = f
        .wait(&run.id, |detail| {
            matches!(
                detail.run.state,
                RunState::Failed | RunState::Interrupted | RunState::Expired
            )
        })
        .await;
    assert!(done.file_operations.is_empty() && done.run.model_attempts.is_empty());
    assert!(!f.original().join("report.docx").exists());
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(stored.progress.charged_tokens(), 0);
    assert!(stored.plan.profiles.is_empty());
    worker.close().await;
}

async fn actual_preview(pdf: bool, pages: Vec<u32>) {
    let mut f = Fixture::new(false).await;
    let explicit = !pages.is_empty();
    let data = if pdf {
        let result=std::process::Command::new(python()).args(["-I","-c","import io,sys; from pypdf import PdfWriter; w=PdfWriter(); [w.add_blank_page(width=100,height=100) for _ in range(6)]; b=io.BytesIO(); w.write(b); sys.stdout.buffer.write(b.getvalue())"]).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        result.stdout
    } else {
        base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=").unwrap()
    };
    f.preview_source(if pdf { "source.pdf" } else { "pixel.png" }, &data, pages)
        .await;
    let run = f.commit(AccessMode::ReadOnly).await;
    let worker = Worker::start(f.runtime.clone());
    let done = f.completed(&run.id).await;
    assert!(
        done.file_operations.is_empty()
            && done.approvals.is_empty()
            && done.run.model_attempts.is_empty()
    );
    f.assert_native(&run.id, 1).await;
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let WorkflowNodeState::Completed { output, .. } = &stored.progress.nodes()["read-source"].state
    else {
        panic!("missing preview output");
    };
    let output: serde_json::Value = serde_json::from_str(output).unwrap();
    assert_eq!(output["source"]["sha256"], f.source_sha);
    assert_eq!(output["total_pages"], if pdf { 6 } else { 1 });
    assert_eq!(output["truncated"], pdf && !explicit);
    let images = output["images"].as_array().unwrap();
    assert_eq!(
        images.len(),
        if pdf { if explicit { 2 } else { 4 } } else { 1 }
    );
    if explicit {
        assert_eq!(
            images
                .iter()
                .map(|image| image["page"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![2, 6]
        );
    }
    for image in images {
        assert!(image["width"].as_u64().unwrap() > 0 && image["height"].as_u64().unwrap() > 0);
        let bytes = f
            .runtime
            .run_preview(&run.id, image["sha256"].as_str().unwrap())
            .await
            .unwrap();
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(hash(&bytes), image["sha256"]);
    }
    assert_eq!(
        std::fs::read(f.original().join(&f.source_name)).unwrap(),
        data
    );
    worker.close().await;
}
#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn native_image_preview_is_read_only_and_uses_its_original_run_custody() {
    actual_preview(false, vec![]).await;
}
#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn native_pdf_preview_marks_the_actual_four_of_six_page_limit() {
    actual_preview(true, vec![]).await;
}
#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn native_pdf_preview_keeps_explicit_page_numbers_without_false_truncation() {
    actual_preview(true, vec![6, 2]).await;
}

#[tokio::test]
#[ignore = "requires explicitly configured actual OCR Python and language pack"]
async fn native_ocr_reads_the_confirmed_scan_without_a_model_or_file_mutation() {
    let mut f = Fixture::new(false).await;
    f.ocr_source(true, 0, 100).await;
    let run = f.commit(AccessMode::ReadOnly).await;
    std::fs::write(
        f.original().join("invoice.pdf"),
        b"changed original after copy",
    )
    .unwrap();
    let worker = Worker::start(f.runtime.clone());
    let done = f.completed(&run.id).await;
    assert!(
        done.file_operations.is_empty()
            && done.approvals.is_empty()
            && done.run.model_attempts.is_empty()
    );
    f.assert_native(&run.id, 1).await;
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let WorkflowNodeState::Completed { output, .. } = &stored.progress.nodes()["read-source"].state
    else {
        panic!("missing OCR output")
    };
    let output: serde_json::Value = serde_json::from_str(output).unwrap();
    assert_eq!(output["source"]["sha256"], f.source_sha);
    assert!(output["text"].as_str().unwrap().contains("119,00"));
    assert_eq!(output["engine"]["languages"], json!(["deu", "eng"]));
    assert_eq!(output["truncated"], false);
    assert!(output["next_offset"].is_null());
    for page in output["pages"].as_array().unwrap() {
        let sha = page["preview_sha256"].as_str().unwrap();
        let bytes = f.runtime.run_preview(&run.id, sha).await.unwrap();
        assert_eq!(hash(&bytes), sha);
    }
    assert_eq!(
        std::fs::read(f.original().join("invoice.pdf")).unwrap(),
        b"changed original after copy"
    );
    worker.close().await;
}

#[tokio::test]
#[ignore = "requires explicitly configured actual OCR Python and language pack"]
async fn native_ocr_keeps_word_continuation_and_positions_in_its_typed_output() {
    let mut f = Fixture::new(false).await;
    f.ocr_source(true, 2, 3).await;
    let run = f.commit(AccessMode::ReadOnly).await;
    let worker = Worker::start(f.runtime.clone());
    f.completed(&run.id).await;
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let WorkflowNodeState::Completed { output, .. } = &stored.progress.nodes()["read-source"].state
    else {
        panic!("missing OCR output")
    };
    let output: serde_json::Value = serde_json::from_str(output).unwrap();
    assert_eq!(output["offset"], 2);
    assert_eq!(output["next_offset"], 5);
    assert_eq!(output["words"].as_array().unwrap().len(), 3);
    assert_eq!(output["words"][0]["index"], 2);
    assert_eq!(output["truncated"], true);
    worker.close().await;
}

#[tokio::test]
#[ignore = "requires explicitly configured actual OCR Python"]
async fn native_ocr_missing_language_pack_is_rejected_before_a_plan_or_run_is_accepted() {
    let mut f = Fixture::new(false).await;
    f.ocr_source(false, 0, 100).await;
    let error = f
        .runtime
        .prepare_execution_plan_v2(&f.request(AccessMode::ReadOnly).await)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("workflow_document_ocr_unavailable"),
        "{error:?}"
    );
    assert!(f.runtime.store.runs(&f.order.id).await.unwrap().is_empty());
}

async fn lost_native_publication(source_ack: bool) {
    let f = Fixture::new(false).await;
    let run = f.commit(AccessMode::Autonomous).await;
    let path = f.temp.path().join("native-documents.sqlite3");
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let trigger = if source_ack {
        "CREATE TRIGGER lose_native_ack BEFORE UPDATE ON workflow_external_evidence WHEN json_extract(NEW.payload,'$.kind')='file_operation' AND NEW.result_sha256 IS NOT NULL BEGIN SELECT RAISE(ABORT,'lost native source outcome'); END"
    } else {
        "CREATE TRIGGER lose_native_ack BEFORE INSERT ON events WHEN json_extract(NEW.payload,'$.kind')='workflow_native_tool_completed' AND json_extract(NEW.payload,'$.payload.node_id')='write-report' BEGIN SELECT RAISE(ABORT,'lost native graph outcome'); END"
    };
    sqlx::query(trigger).execute(&pool).await.unwrap();
    let interrupted = f
        .runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await;
    assert!(
        interrupted.is_err(),
        "the publication interruption was not injected"
    );
    assert_eq!(
        f.runtime.store.detail(&run.id).await.unwrap().run.state,
        RunState::Interrupted
    );
    let before = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let call = &before
        .progress
        .attempts()
        .values()
        .find(|attempt| {
            attempt.claim.node_id == "write-report"
                && matches!(attempt.operation, WorkflowOperation::Tool { .. })
        })
        .unwrap_or_else(|| {
            panic!(
                "native publication was not reached: result={:?}, nodes={:?}",
                interrupted
                    .as_ref()
                    .map(|record| (&record.state, &record.error_code)),
                before.progress.nodes()
            )
        })
        .id;
    let evidence = f
        .runtime
        .store
        .workflow_external_evidence(&f.project.id, &run.id, call)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(evidence.kind, "file_operation");
    assert_eq!(evidence.result.is_none(), source_ack);
    let expected = evidence.preparation["operation_receipt"]["after"]["sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    let original_bytes = std::fs::read(f.report()).unwrap();
    assert_eq!(hash(&original_bytes), expected);
    sqlx::query("DROP TRIGGER lose_native_ack")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    f.runtime.store.close().await;
    let runtime = Runtime::new(Store::sqlite(&path).await.unwrap(), vec![], false)
        .unwrap()
        .with_file_broker(
            cowork_broker::FileBroker::open(&f.temp.path().join("broker"), vec![]).unwrap(),
        )
        .with_toolbox(f.toolbox.clone());
    let result = runtime
        .reconcile_workflow_effects(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(result["settled_calls"].as_array().unwrap().len(), 1);
    assert!(
        result["unresolved_calls"].as_array().unwrap().is_empty(),
        "{result}"
    );
    let recovered = runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(
        cowork_domain::canonical_hash(&before.plan),
        cowork_domain::canonical_hash(&recovered.plan)
    );
    assert_eq!(
        recovered.agents["write-report"].turns[0].tool_results[0].result["operation_receipt"]["after"]
            ["sha256"],
        expected
    );
    assert_eq!(recovered.agents["write-report"].turns.len(), 1);
    let again = runtime
        .reconcile_workflow_effects(&f.project.id, &run.id)
        .await
        .unwrap();
    assert!(again["settled_calls"].as_array().unwrap().is_empty());
    runtime.store.resume(&run.id).await.unwrap();
    let done = runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(done.state, RunState::Completed, "{:?}", done.state);
    let detail = runtime.store.detail(&run.id).await.unwrap();
    assert!(detail.run.model_attempts.is_empty());
    assert_eq!(detail.file_operations.len(), 1);
    let final_graph = runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(final_graph.progress.charged_tokens(), 0);
    assert!(final_graph.plan.profiles.is_empty());
    assert_eq!(final_graph.agents["write-report"].turns.len(), 1);
    assert_eq!(std::fs::read(f.report()).unwrap(), original_bytes);
    runtime.store.close().await;
}
#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn native_docx_lost_graph_ack_reopens_and_resumes_without_a_second_publication() {
    lost_native_publication(false).await;
}
#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn native_docx_lost_source_outcome_reopens_and_resumes_without_a_second_publication() {
    lost_native_publication(true).await;
}

#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn native_text_chunks_preserve_unicode_and_reassemble_the_original_source() {
    let mut f = Fixture::new(false).await;
    let text = "a".repeat(16_383) + "🦀tail";
    std::fs::write(f.original().join("source.txt"), text.as_bytes()).unwrap();
    f.source_sha = hash(text.as_bytes());
    let mut source = definition(false);
    source.documents.remove("write-report");
    let mut tail = source.documents["read-source"].clone();
    tail.name = "Continue the same frozen source".into();
    tail.dependencies = vec!["read-source".into()];
    tail.operation = WorkflowDocumentOperation::ReadText {
        path_input: "path".into(),
        sha256_input: "sha".into(),
        offset: 16_384,
    };
    source.documents.insert("read-tail".into(), tail);
    let flow = source.dataflow.as_mut().unwrap();
    flow.steps.remove("write-report");
    flow.steps
        .insert("read-tail".into(), flow.steps["read-source"].clone());
    let entry = f
        .runtime
        .store
        .create_workflow(&f.project.id, "native-chunk-source", &source)
        .await
        .unwrap();
    f.workflow = entry.id;
    f.version = entry.versions[0].id.clone();
    let run = f.commit(AccessMode::ReadOnly).await;
    let worker = Worker::start(f.runtime.clone());
    let done = f.completed(&run.id).await;
    assert!(
        done.file_operations.is_empty()
            && done.approvals.is_empty()
            && done.run.model_attempts.is_empty()
    );
    f.assert_native(&run.id, 2).await;
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let result = |node: &str| -> serde_json::Value {
        let WorkflowNodeState::Completed { output, .. } = &graph.progress.nodes()[node].state
        else {
            panic!("missing native read result");
        };
        serde_json::from_str(output).unwrap()
    };
    let first = result("read-source");
    let last = result("read-tail");
    assert_eq!(first["next_offset"], 16_384);
    assert_eq!(first["truncated"], true);
    assert_eq!(last["offset"], 16_384);
    assert_eq!(last["truncated"], false);
    assert!(last["next_offset"].is_null());
    assert_eq!(
        first["text"].as_str().unwrap().to_owned() + last["text"].as_str().unwrap(),
        text
    );
    worker.close().await;
}

#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn a_native_write_cannot_be_prepared_with_read_only_rights() {
    let f = Fixture::new(false).await;
    let request = f.request(AccessMode::ReadOnly).await;
    assert!(matches!(
        f.runtime.prepare_execution_plan_v2(&request).await,
        Err(cowork_domain::Error::Forbidden)
    ));
    assert!(f.runtime.store.runs(&f.order.id).await.unwrap().is_empty());
    assert!(!f.original().join("report.docx").exists());
}

#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn a_native_source_hash_mismatch_never_reaches_the_report_action() {
    let mut f = Fixture::new(false).await;
    f.source_sha = "a".repeat(64);
    let run = f.commit(AccessMode::Autonomous).await;
    let worker = Worker::start(f.runtime.clone());
    let detail = f
        .wait(&run.id, |d| {
            matches!(d.run.state, RunState::Failed | RunState::Interrupted)
        })
        .await;
    assert!(
        detail.file_operations.is_empty()
            && detail.approvals.is_empty()
            && detail.run.model_attempts.is_empty()
    );
    let graph = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert!(!graph.agents.contains_key("write-report"));
    assert_eq!(graph.progress.charged_tokens(), 0);
    assert!(!f.original().join("report.docx").exists());
    worker.close().await;
}

#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn a_denied_native_report_approval_never_publishes_a_document() {
    let f = Fixture::new(false).await;
    let run = f.commit(AccessMode::ConfirmMutations).await;
    let worker = Worker::start(f.runtime.clone());
    let approval = f.approval(&run.id).await;
    f.runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint,
                allow: false,
            },
        )
        .await
        .unwrap();
    let detail = f
        .wait(&run.id, |d| {
            matches!(d.run.state, RunState::Failed | RunState::Interrupted)
        })
        .await;
    assert!(detail.file_operations.is_empty() && detail.run.model_attempts.is_empty());
    assert_eq!(detail.approvals.len(), 1);
    assert_eq!(detail.approvals[0].decision, Some(false));
    assert!(!f.original().join("report.docx").exists());
    worker.close().await;
}
