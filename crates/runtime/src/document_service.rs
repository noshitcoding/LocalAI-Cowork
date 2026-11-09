//! Shared bounded document preparation; publication remains with each caller journal.
use super::*;
use cowork_broker::GeneratedContent;
use cowork_domain::documents::{DocumentCall, parse_document_call};
use std::sync::atomic::{AtomicBool, Ordering};

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(super) fn failure(error: &Error) -> Value {
    json!({"error":error.code(),"message":error.to_string()})
}

fn engine_error(error: cowork_toolbox::Error) -> Error {
    match error {
        cowork_toolbox::Error::Invalid(message) => Error::Validation(message),
        _ => Error::Validation("The document engine failed".into()),
    }
}

/// The output folder: explicit, or the run's managed working folder.
pub(super) fn output_target(
    run: &RunRecord,
    call: &DocumentCall,
) -> Result<Option<(String, String)>> {
    let Some(output) = &call.output else {
        return Ok(None);
    };
    let folder = match &output.folder_id {
        Some(id) => id.clone(),
        None => run
            .spec
            .folders
            .iter()
            .find(|f| f.writable && f.order_id.as_deref() == Some(run.spec.order_id.as_str()))
            .map(|f| f.id.clone())
            .ok_or_else(|| {
                Error::Validation("Choose output.folder_id; this run has no working folder".into())
            })?,
    };
    if !run
        .spec
        .folders
        .iter()
        .any(|f| f.id == folder && f.writable)
    {
        return Err(Error::Validation(
            "output.folder_id is not a writable granted folder".into(),
        ));
    }
    Ok(Some((folder, output.path.clone())))
}

pub(super) fn validate_document_scope(run: &RunRecord, call: &DocumentCall) -> Result<()> {
    let granted = |id: &str| run.spec.folders.iter().any(|folder| folder.id == id);
    if call.inputs.iter().any(|input| !granted(&input.folder_id))
        || call
            .output
            .as_ref()
            .and_then(|output| output.folder_id.as_ref())
            .is_some_and(|id| !granted(id))
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

pub(super) struct DocumentPublication {
    pub folder_id: String,
    pub path: String,
    pub content: GeneratedContent,
    pub provenance: DocumentProvenance,
}

/// Engine result only. The caller must use its own durable effect journal before
/// publishing; this value does not admit or apply a filesystem operation.
pub(super) struct PreparedDocumentExecution {
    pub result: Value,
    pub images: Vec<Value>,
    pub publication: Option<DocumentPublication>,
    pub succeeded: bool,
    pub engine_revision: String,
}

impl Runtime {
    /// Shared stage/input/engine/preview path for chat, human actions and workflows.
    /// The outer error means ownership/cancellation/deadline failed while waiting;
    /// the inner error is a confirmed validation/engine failure with no publication.
    pub(super) async fn prepare_document_execution(
        &self,
        scope: &RunRecord,
        token: &str,
        tool: &ToolCall,
        stop: &CancellationToken,
        deadline: tokio::time::Instant,
        expected_engine_revision: Option<&str>,
    ) -> Result<Result<PreparedDocumentExecution>> {
        let call = match parse_document_call(tool) {
            Ok(call) => call,
            Err(error) => return Ok(Err(error)),
        };
        if let Err(error) = validate_document_scope(scope, &call) {
            return Ok(Err(error));
        }
        let output = match output_target(scope, &call) {
            Ok(output) => output,
            Err(error) => return Ok(Err(error)),
        };
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let toolbox = self.toolbox.clone().ok_or(Error::Forbidden)?;
        let run = scope.clone();
        let requested = call.inputs.clone();
        let task = tokio::task::spawn_blocking(
            move || -> Result<Vec<cowork_toolbox::DocumentInputFile>> {
                requested
                    .into_iter()
                    .map(|input| {
                        let location = FileLocation {
                            folder_id: input.folder_id.clone(),
                            path: input.path.clone(),
                        };
                        let bytes = files.read_binary(
                            &run,
                            &location,
                            cowork_toolbox::MAX_DOCUMENT_INPUT_BYTES,
                        )?;
                        let sha256 = hash(&bytes);
                        if input
                            .expected_sha256
                            .as_ref()
                            .is_some_and(|expected| *expected != sha256)
                        {
                            return Err(Error::FileConflict);
                        }
                        Ok(cowork_toolbox::DocumentInputFile {
                            folder_id: input.folder_id,
                            path: input.path,
                            sha256,
                            bytes,
                        })
                    })
                    .collect()
            },
        );
        let inputs = match self
            .wait_file_task(&scope.id, token, stop, deadline, task)
            .await?
        {
            Ok(inputs) => inputs,
            Err(error) => return Ok(Err(error)),
        };
        let provenance_inputs: Vec<DocumentProvenanceInput> = inputs
            .iter()
            .map(|i| DocumentProvenanceInput {
                folder_id: i.folder_id.clone(),
                path: i.path.clone(),
                sha256: i.sha256.clone(),
            })
            .collect();
        let engine_revision = expected_engine_revision
            .map(str::to_owned)
            .or_else(|| {
                if scope.spec.file_action.as_ref() == Some(tool) {
                    tool.id.strip_prefix("document-action:").map(str::to_owned)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| toolbox.document_engine_revision());
        let job = cowork_toolbox::DocumentJob {
            engine_revision: Some(engine_revision.clone()),
            tool: call.tool.clone(),
            operation: call.operation.clone(),
            inputs,
            output_name: output
                .as_ref()
                .map(|(_, path)| path.rsplit('/').next().unwrap_or(path).to_owned()),
            output_directory: call.output_is_directory(),
            spec: call.spec.clone(),
            options: call.options.clone(),
            timeout: deadline
                .saturating_duration_since(tokio::time::Instant::now())
                .min(Duration::from_secs(300)),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let _cancel = CancelOnDrop(cancel.clone());
        let task = tokio::task::spawn_blocking(move || {
            toolbox.execute_document(job, cancel).map_err(engine_error)
        });
        let outcome = match self
            .wait_file_task(&scope.id, token, stop, deadline, task)
            .await?
        {
            Ok(outcome) => outcome,
            Err(error) => return Ok(Err(error)),
        };
        let mut result = outcome.receipt.clone();
        let mut images = Vec::new();
        if !outcome.previews.is_empty() {
            let files = self.files.clone().ok_or(Error::Forbidden)?;
            let previews = outcome.previews.clone();
            let task = tokio::task::spawn_blocking(move || {
                previews
                    .iter()
                    .map(|p| files.retain_preview(&p.bytes))
                    .collect::<Result<Vec<_>>>()
            });
            if let Ok(retained) = self
                .wait_file_task(&scope.id, token, stop, deadline, task)
                .await?
            {
                images = outcome
                    .previews
                    .iter()
                    .zip(retained)
                    .map(|(p, sha256)| json!({"sha256":sha256,"media_type":"image/png","page":p.page,"width":p.width,"height":p.height}))
                    .collect();
            }
            let vision = scope
                .spec
                .provider
                .as_ref()
                .is_some_and(|p| p.capabilities.image_input)
                && scope.spec.model_binding.is_none();
            result["previews"] = json!({
                "count": outcome.previews.len(),
                "shown_to_model": vision,
                "note": if vision { "The rendered images follow in the next message." } else { "This model has no image input; use the text, inspect or metadata results instead." },
            });
        }
        if !outcome.ok {
            let mut failed = json!({
                "error": result["error"]["code"].as_str().unwrap_or("engine"),
                "message": result["error"]["message"].as_str().unwrap_or("The document engine failed"),
            });
            for key in ["diagnostics", "previews"] {
                if let Some(value) = result.get(key) {
                    failed[key] = value.clone();
                }
            }
            if let Some(path) = result["error"].get("path") {
                failed["path"] = path.clone();
            }
            return Ok(Ok(PreparedDocumentExecution {
                result: failed,
                images,
                publication: None,
                succeeded: false,
                engine_revision,
            }));
        }
        let Some((folder_id, path)) = output else {
            return Ok(Ok(PreparedDocumentExecution {
                result,
                images,
                publication: None,
                succeeded: true,
                engine_revision,
            }));
        };
        let content = if call.output_is_directory() {
            GeneratedContent::Directory(
                outcome
                    .outputs
                    .iter()
                    .map(|f| (f.path.clone(), f.bytes.clone()))
                    .collect(),
            )
        } else {
            match outcome.outputs.first() {
                Some(file) => GeneratedContent::File(file.bytes.clone()),
                None => {
                    let error = Error::Validation("The document engine returned no output".into());
                    return Ok(Err(error));
                }
            }
        };
        let provenance = DocumentProvenance {
            tool: call.tool.clone(),
            operation: call.operation.clone(),
            engine_version: outcome.engine_version.clone(),
            inputs: provenance_inputs,
            receipt_sha256: outcome.receipt_sha256.clone(),
        };
        Ok(Ok(PreparedDocumentExecution {
            result,
            images,
            succeeded: true,
            engine_revision,
            publication: Some(DocumentPublication {
                folder_id,
                path,
                content,
                provenance,
            }),
        }))
    }
}
