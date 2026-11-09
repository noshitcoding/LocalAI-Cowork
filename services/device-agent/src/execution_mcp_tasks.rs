use super::*;
use cowork_mcp::{TaskOperation, TaskOperationResult, TaskStatus};
#[expect(
    clippy::too_many_arguments,
    reason = "MCP task continuation keeps the run lease, original source, session, journal and cancellation authority explicit."
)]
pub(super) async fn await_task(
    engine: &ExecutionEngine,
    control: &dyn DeviceControl,
    run: Uuid,
    lease: Uuid,
    source: Uuid,
    binding: &McpBinding,
    session: &mut Session,
    mut result: cowork_mcp::CallResult,
    journal: &journal::Journal,
    stop: &CancellationToken,
) -> Result<cowork_mcp::CallResult> {
    let mut ack = result
        .task
        .take()
        .context("device_mcp_original_task_missing")?;
    let original = ack.clone();
    let now = chrono::Utc::now().timestamp_millis();
    let deadline = ack
        .expires_at_ms()
        .unwrap_or(now.saturating_add(86_400_000))
        .min(now.saturating_add(86_400_000));
    ensure!(
        deadline > now,
        "device_mcp_original_task_expired_effect_unknown"
    );
    journal.put("task/ack","task_ack",&json!({"original":ack,"response":result.response,"response_sha256":result.response_sha256,"connection_id":session.connection_id(),"connection_epoch":session.epoch(),"protocol":session.protocol_version(),"lease_sha256":digest(lease.as_bytes()),"deadline":deadline}))?;
    let cancel_prompt = json!({"format":"device-mcp-task-cancel/1","question":"This original MCP task is still running. To request cancellation choose Cancel task. Cancellation does not prove absence of remote changes.","task_record_id":source,"connection":binding.name,"original_task_sha256":digest(ack.task_id().as_bytes()),"effects_unknown":true});
    let control_source = Uuid::new_v4();
    journal.put(
        "task/cancel-control-intent",
        "task_response",
        &json!({"source":control_source,"prompt":cancel_prompt}),
    )?;
    let input=control.run_json(run,lease,"POST","input-requests",json!({"lease_token":lease,"source_request_id":control_source,"prompt":cancel_prompt,"expires_at":chrono::DateTime::from_timestamp_millis(deadline).context("device_mcp_deadline_invalid")?.to_rfc3339()})).await?;
    let cancel_id = uuid(&input, "id")?;
    journal.put("task/cancel-control-received", "task_response", &input)?;
    let mut index = 0u32;
    let mut canceled = false;
    loop {
        interactions::current(control, run, lease, deadline, stop).await?;
        let cancellation = control
            .run_json(
                run,
                lease,
                "GET",
                &format!("input-requests/{cancel_id}"),
                Value::Null,
            )
            .await?;
        ensure!(
            cancellation["prompt"] == cancel_prompt,
            "device_mcp_cancel_control_changed"
        );
        let cancel = cancellation["state"] == "submitted"
            && cancellation["response"].as_str().is_some_and(|s| {
                s == "cancel"
                    || serde_json::from_str::<Value>(s).is_ok_and(|v| v["action"] == "cancel")
            });
        let operation = if cancel && !canceled && !ack.status.terminal() {
            canceled = true;
            journal.put("task/cancel-decision", "task_response", &cancellation)?;
            TaskOperation::Cancel
        } else if ack.status.terminal() || ack.status == TaskStatus::InputRequired {
            TaskOperation::Result
        } else {
            TaskOperation::Get
        };
        let remaining = u64::try_from(deadline - chrono::Utc::now().timestamp_millis())
            .map_err(|_| anyhow::anyhow!("device_mcp_task_expired_effect_unknown"))?;
        let prepared = session
            .prepare_task_operation_bounded(&ack, operation, remaining)
            .map_err(|_| anyhow::anyhow!("device_mcp_task_original_session_invalid"))?;
        let operation_id = Uuid::new_v4();
        let key = format!("task/rpc/{index}");
        index = index
            .checked_add(1)
            .context("device_mcp_task_operation_bound")?;
        ensure!(index <= 4096, "device_mcp_task_operation_bound");
        journal.put(&format!("{key}/intent"),"task_response",&json!({"operation_id":operation_id,"operation":match operation{TaskOperation::Get=>"tasks/get",TaskOperation::Result=>"tasks/result",TaskOperation::Cancel=>"tasks/cancel"},"connection_id":session.connection_id(),"epoch":session.epoch(),"rpc_id":prepared.rpc_request_id(),"request_sha256":prepared.request_sha256(),"original_task":original,"current_task":ack,"deadline":deadline}))?;
        let handler = interactions::Handler {
            engine,
            control,
            run,
            lease,
            source,
            session_id: session.connection_id(),
            epoch: session.epoch(),
            task_id: ack.task_id().to_string(),
            operation_rpc: prepared.rpc_request_id().to_string(),
            operation_id,
            journal,
            deadline,
            stop,
        };
        interactions::current(control, run, lease, deadline, stop).await?;
        let reply = tokio::select! {_=stop.cancelled()=>bail!("device_mcp_task_canceled_effect_unknown_no_replay"),r=session.task_operation_interactive(prepared,&ack,&handler)=>r};
        match reply {
            Ok(TaskOperationResult::Status(next)) => {
                journal.put(
                    &format!("{key}/received"),
                    "task_response",
                    &json!({"kind":"status","response":next.original()}),
                )?;
                ack = next;
            }
            Ok(TaskOperationResult::Result(final_result)) => {
                journal.put(&format!("{key}/received"),"task_response",&json!({"kind":"result","response":final_result.response,"sha256":final_result.response_sha256}))?;
                interactions::current(control, run, lease, deadline, stop).await?;
                return Ok(final_result);
            }
            Ok(TaskOperationResult::Error { response, sha256 }) => {
                journal.put(&format!("{key}/received"),"task_response",&json!({"kind":"error","response":response,"sha256":sha256,"effects_unknown":true}))?;
                bail!("device_mcp_original_task_error_effect_unknown_no_replay");
            }
            Err(error) => {
                if let Some(response) = error.response() {
                    journal.put(
                        &format!("{key}/unconfirmed-response"),
                        "task_response",
                        &json!({"response":response,"effects_unknown":true}),
                    )?;
                }
                bail!("device_mcp_task_rpc_unconfirmed_no_replay");
            }
        }
        engine.event(control,run,lease,"mcp_task_status",json!({"task_record_id":source,"connection":binding.name,"state":ack.status,"effects_unknown":true,"original_receipt_local":true})).await?;
        if !ack.status.terminal() && ack.status != TaskStatus::InputRequired {
            tokio::select! {_=stop.cancelled()=>bail!("device_mcp_task_canceled_effect_unknown_no_replay"),_=tokio::time::sleep(Duration::from_millis(ack.poll_interval_ms.min(60_000)))=>{}}
        }
    }
}
