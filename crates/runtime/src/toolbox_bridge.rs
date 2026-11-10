//! Explicit native-tool capability, separate from qualified remote ProcessRunner.
use super::*;
use cowork_toolbox::{Action, Request, Toolbox};
use serde::Deserialize;

pub(crate) const NATIVE_TOOLS: [&str; 4] = ["execute_program", "browser", "office", "github"];
pub(crate) fn is_native_tool(name: &str) -> bool {
    NATIVE_TOOLS.contains(&name)
}
fn invalid() -> Error {
    Error::Validation("Invalid native tool request".into())
}
fn tool_error(_: cowork_toolbox::Error) -> Error {
    Error::UncertainEffect
}
pub(super) fn job_result(job: &cowork_toolbox::Job) -> Value {
    let outcome = job.outcome.as_ref();
    let capture =
        |bytes: &[u8]| String::from_utf8_lossy(&bytes[..bytes.len().min(24 * 1024)]).into_owned();
    json!({"job_id":job.id,"state":job.state,"outcome":outcome.map(|o|json!({"exit_code":o.exit_code,"termination":o.termination,"stdout":capture(&o.stdout.bytes),"stderr":capture(&o.stderr.bytes),"stdout_truncated":o.stdout.truncated||o.stdout.bytes.len()>24*1024,"stderr_truncated":o.stderr.truncated||o.stderr.bytes.len()>24*1024,"output_complete":o.output_complete,"duration_ms":o.duration_ms}))})
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Program {
    program: String,
    arguments: Vec<String>,
    timeout_seconds: u64,
    #[serde(default)]
    cwd: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Browser {
    #[serde(default)]
    engine: Option<BrowserEngine>,
    #[serde(default)]
    mode: Option<BrowserMode>,
    #[serde(default)]
    connection_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    operation: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    text: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Office {
    operation: String,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    title: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Github {
    arguments: Vec<String>,
}
struct NativeJobGuard {
    toolbox: Arc<Toolbox>,
    order: String,
    job: String,
}
impl Drop for NativeJobGuard {
    fn drop(&mut self) {
        let _ = self.toolbox.cancel(&self.order, &self.job);
    }
}
impl Runtime {
    pub fn with_toolbox(mut self, service: Arc<Toolbox>) -> Self {
        self.toolbox = Some(service);
        self
    }
    pub(crate) fn selected_native_tools(&self, tools: &[String]) -> Result<Vec<String>> {
        if tools.is_empty() {
            return Ok(Vec::new());
        }
        let service = self.toolbox.as_ref().ok_or(Error::Forbidden)?;
        let available = service.availability();
        let mut selected = std::collections::BTreeSet::new();
        for name in tools {
            let ready = match name.as_str() {
                "execute_program" => !available.programs.is_empty(),
                "browser" => available.browser_available,
                "office" => available.programs.iter().any(|p| p == "python"),
                "github" => available.github_available,
                _ => false,
            };
            if !ready || !selected.insert(name.clone()) {
                return Err(invalid());
            }
        }
        Ok(selected.into_iter().collect())
    }
    pub(crate) fn native_request(&self, record: &RunRecord, tool: &ToolCall) -> Result<Request> {
        let service = self.toolbox.as_ref().ok_or(Error::Forbidden)?;
        let remaining = remaining_run_millis(record, now());
        if remaining < 1000 {
            return Err(Error::BudgetExceeded);
        }
        let (action, timeout) = match tool.name.as_str() {
            "execute_program" => {
                if record.spec.environment.is_some() {
                    return Err(Error::Forbidden);
                }
                let v: Program =
                    serde_json::from_value(tool.arguments.clone()).map_err(|_| invalid())?;
                if v.timeout_seconds == 0 || v.timeout_seconds > 600 {
                    return Err(invalid());
                }
                (
                    Action::Terminal {
                        program: v.program,
                        arguments: v.arguments,
                        cwd: v.cwd,
                    },
                    v.timeout_seconds,
                )
            }
            "browser" => {
                let v: Browser =
                    serde_json::from_value(tool.arguments.clone()).map_err(|_| invalid())?;
                let session_id = match v.session_id {
                    Some(s) => s,
                    None if v.engine.is_some() || v.mode.is_some() => {
                        service
                            .create_browser_v2(
                                &record.spec.project_id,
                                &record.spec.order_id,
                                CreateBrowserSession {
                                    engine: v.engine.unwrap_or_default(),
                                    mode: v.mode.unwrap_or_default(),
                                    connection_id: v.connection_id,
                                },
                            )
                            .map_err(tool_error)?
                            .id
                    }
                    None if service
                        .selected_browser_session(&record.spec.order_id)
                        .is_some() =>
                    {
                        service
                            .selected_browser_session(&record.spec.order_id)
                            .ok_or_else(invalid)?
                    }
                    None => {
                        service
                            .create_browser_session(&record.spec.order_id)
                            .map_err(tool_error)?
                            .id
                    }
                };
                (
                    Action::Browser {
                        session_id,
                        operation: v.operation,
                        url: v.url,
                        selector: v.selector,
                        text: v.text,
                    },
                    120,
                )
            }
            "office" => {
                let v: Office =
                    serde_json::from_value(tool.arguments.clone()).map_err(|_| invalid())?;
                (
                    Action::Office {
                        operation: v.operation,
                        input: v.input,
                        output: v.output,
                        text: v.text,
                        title: v.title,
                    },
                    120,
                )
            }
            "github" => {
                let v: Github =
                    serde_json::from_value(tool.arguments.clone()).map_err(|_| invalid())?;
                (
                    Action::Github {
                        arguments: v.arguments,
                    },
                    120,
                )
            }
            _ => return Err(Error::Forbidden),
        };
        Ok(Request {
            idempotency_key: canonical_hash(
                &json!({"namespace":"native-model-tool/v1","run":record.id,"tool":tool,"approval":approval_fingerprint(record,tool)}),
            ),
            action,
            timeout_seconds: timeout.min(remaining / 1000),
            output_bytes: 128 * 1024,
            approval_sha256: None,
        })
    }
    pub(crate) async fn invoke_native_toolbox(
        &self,
        run: &str,
        lease: &str,
        tool: &ToolCall,
        stop: &CancellationToken,
    ) -> Result<()> {
        let service = self.toolbox.as_ref().ok_or(Error::Forbidden)?.clone();
        let before = self.store.stored_run(run).await?;
        before.assert_lease(lease, now())?;
        ensure_running(&before)?;
        ensure_pending(&before, tool)?;
        let mut request = self.native_request(&before.record, tool)?;
        let preview = service
            .preview(&before.record.spec.order_id, &request)
            .map_err(tool_error)?;
        self.store.mutate(run,Some(lease),|r|{ensure_running(r)?;ensure_pending(r,tool)?;native_exact_approval(&r.record,tool,&r.checkpoint.approvals)?;if r.checkpoint.in_flight.is_some(){return Err(Error::UncertainEffect);}r.checkpoint.in_flight=Some(tool.clone());Ok(Change::event("native_tool_started",json!({"tool_call_id":tool.id,"name":tool.name,"request_sha256":preview.request_sha256,"mode":"owner_native","filesystem_isolation":false}))) }).await?;
        let current = self.store.stored_run(run).await?;
        current.assert_lease(lease, now())?;
        ensure_running(&current)?;
        native_exact_approval(&current.record, tool, &current.checkpoint.approvals)?;
        if current.checkpoint.in_flight.as_ref() != Some(tool) || stop.is_cancelled() {
            return Err(Error::UncertainEffect);
        }
        request.approval_sha256 = Some(preview.request_sha256);
        let job = service
            .submit(&current.record.spec.order_id, request)
            .map_err(tool_error)?;
        let _guard = NativeJobGuard {
            toolbox: service.clone(),
            order: current.record.spec.order_id.clone(),
            job: job.id.clone(),
        };
        let mut beat = tokio::time::Instant::now();
        let result = loop {
            let current = self.store.stored_run(run).await?;
            current.assert_lease(lease, now())?;
            ensure_running(&current)?;
            native_exact_approval(&current.record, tool, &current.checkpoint.approvals)?;
            if stop.is_cancelled() || current.checkpoint.in_flight.as_ref() != Some(tool) {
                return Err(Error::UncertainEffect);
            }
            if beat.elapsed() >= Duration::from_secs(3) {
                self.store.heartbeat(run, lease).await?;
                beat = tokio::time::Instant::now();
            }
            let state = service
                .status(&current.record.spec.order_id, &job.id)
                .map_err(tool_error)?;
            if state.state == "completed" {
                break job_result(&state);
            }
            if state.state != "running" {
                return Err(Error::UncertainEffect);
            }
            tokio::select! {_=stop.cancelled()=>return Err(Error::UncertainEffect),_=tokio::time::sleep(Duration::from_millis(100))=>{}}
        };
        self.store
            .mutate(run, Some(lease), |r| {
                ensure_running(r)?;
                ensure_pending(r, tool)?;
                native_exact_approval(&r.record, tool, &r.checkpoint.approvals)?;
                if r.checkpoint.in_flight.as_ref() != Some(tool) {
                    return Err(Error::UncertainEffect);
                }
                r.checkpoint.in_flight = None;
                r.checkpoint.pending_tools.remove(0);
                r.checkpoint.messages.push(
                    json!({"role":"tool","tool_call_id":tool.id,"content":result.to_string()}),
                );
                r.record.steps += 1;
                Ok(Change::event(
                    "native_tool_completed",
                    json!({"tool_call_id":tool.id,"name":tool.name,"result":result}),
                ))
            })
            .await?;
        Ok(())
    }
    pub(crate) async fn invoke_workflow_toolbox(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn: &str,
        raw: &str,
        stop: &CancellationToken,
    ) -> Result<cowork_storage::WorkflowToolExecution> {
        use cowork_storage::workflow_toolbox::NativeWorkflowAdmission;
        let service = self
            .toolbox
            .as_ref()
            .ok_or_else(|| Error::Provider("workflow_tools_unavailable".into()))?
            .clone();
        let admitted = self
            .store
            .admit_workflow_toolbox(project, run, lease, turn, raw)
            .await?;
        let (record, tool) = match admitted {
            NativeWorkflowAdmission::Dispatch { record, tool } => (*record, tool),
            NativeWorkflowAdmission::Completed(r) => {
                return Ok(cowork_storage::WorkflowToolExecution::Completed(r));
            }
            NativeWorkflowAdmission::AwaitingApproval(a) => {
                return Ok(cowork_storage::WorkflowToolExecution::AwaitingApproval(a));
            }
        };
        let mut request = self.native_request(&record, &tool)?;
        let preview = service
            .preview(&record.spec.order_id, &request)
            .map_err(tool_error)?;
        self.store
            .assert_workflow_toolbox_dispatch(project, run, lease, turn, raw)
            .await?;
        if stop.is_cancelled() {
            return Err(Error::UncertainEffect);
        }
        request.approval_sha256 = Some(preview.request_sha256.clone());
        self.store
            .retain_workflow_external_preparation(
                project,
                run,
                turn,
                raw,
                "native_toolbox",
                json!({"request":request,"request_sha256":preview.request_sha256,"order_id":record.spec.order_id}),
            )
            .await?;
        self.store
            .assert_workflow_toolbox_dispatch(project, run, lease, turn, raw)
            .await?;
        if stop.is_cancelled() {
            return Err(Error::UncertainEffect);
        }
        let job = service
            .submit(&record.spec.order_id, request)
            .map_err(tool_error)?;
        let _guard = NativeJobGuard {
            toolbox: service.clone(),
            order: record.spec.order_id.clone(),
            job: job.id.clone(),
        };
        let result = loop {
            self.store
                .assert_workflow_toolbox_dispatch(project, run, lease, turn, raw)
                .await?;
            if stop.is_cancelled() {
                return Err(Error::UncertainEffect);
            }
            let state = service
                .status(&record.spec.order_id, &job.id)
                .map_err(tool_error)?;
            if state.state == "completed" {
                break job_result(&state);
            }
            if state.state != "running" {
                return Err(Error::UncertainEffect);
            }
            tokio::select! {_=stop.cancelled()=>return Err(Error::UncertainEffect),_=tokio::time::sleep(Duration::from_millis(100))=>{}}
        };
        let mut result = result;
        result["_workflow_success"] = json!(
            result.pointer("/outcome/exit_code").and_then(Value::as_i64) == Some(0)
                && result
                    .pointer("/outcome/termination")
                    .and_then(Value::as_str)
                    == Some("exited")
                && result.get("state").and_then(Value::as_str) == Some("completed")
        );
        self.store
            .retain_workflow_external_outcome(project, run, &tool.id, result.clone())
            .await?;
        self.store
            .finish_workflow_toolbox(project, run, lease, turn, raw, result)
            .await
            .map(cowork_storage::WorkflowToolExecution::Completed)
    }
}
pub(crate) fn native_exact_approval(
    record: &RunRecord,
    tool: &ToolCall,
    approvals: &[Approval],
) -> Result<()> {
    if !record.spec.allowed_tools.contains(&tool.name)
        || record.spec.access_mode == AccessMode::ReadOnly
        || evaluate(record, tool, &[], &[], approvals) != Decision::Allow
        || !approvals.iter().any(|a| {
            a.run_id == record.id
                && a.tool == *tool
                && a.target == record.spec.executor
                && a.policy_revision == record.spec.policy_revision
                && a.fingerprint == approval_fingerprint(record, tool)
                && a.decision == Some(true)
        })
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
