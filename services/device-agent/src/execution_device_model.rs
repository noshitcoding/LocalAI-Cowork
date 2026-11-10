//! One original provider request, shared by the ordinary loop and the real Crew bridge.
use super::*;
pub(super) async fn turn(
    engine: &ExecutionEngine,
    control: &dyn DeviceControl,
    run: Uuid,
    lease: Uuid,
    key: &str,
    body: Value,
    stop: &CancellationToken,
) -> Result<Value> {
    run_configuration::check_deadline()?;
    mcp::interactions::model_budget(engine, run)?;
    let model = engine
        .config
        .model
        .as_ref()
        .context("device_own_model_not_configured")?;
    let endpoint = engine
        .model_endpoint
        .clone()
        .context("device_own_model_not_configured")?;
    let mut wire = body.clone();
    let settings = run_configuration::current(engine);
    let reserved_tokens = settings.apply(&mut wire, model.api == "ollama")?;
    settings.reserve(engine, run, reserved_tokens)?;
    if model.api == "ollama" {
        wire.as_object_mut()
            .context("device_model_request_invalid")?
            .remove("tool_choice");
        if let Some(format) = wire.as_object_mut().unwrap().remove("response_format") {
            wire["format"] = if format["type"] == "json_schema" {
                format["json_schema"]["schema"].clone()
            } else {
                json!("json")
            };
        }
    }
    let sha = digest(&serde_json::to_vec(&wire)?);
    ensure!(
        serde_json::to_vec(&wire)?.len() <= 4 * 1024 * 1024,
        "device_model_context_limit_reached"
    );
    engine.admit(run,key,json!({"phase":"model_dispatched","scope":key,"endpoint":endpoint.as_str(),"model_api":model.api,"request_sha256":sha,"reserved_tokens":reserved_tokens}))?;
    engine
        .event(
            control,
            run,
            lease,
            "model_started",
            json!({"model":model.model,"scope":key,"adapter":model.implementation}),
        )
        .await?;
    let timeout = Duration::from_millis(
        settings
            .agent
            .generation
            .timeout_ms
            .unwrap_or(model.timeout_seconds.saturating_mul(1000))
            .min(model.timeout_seconds.saturating_mul(1000)),
    );
    let mut request = engine
        .model_http
        .post(endpoint)
        .timeout(timeout)
        .json(&wire);
    if let Some(token) = &model.api_key {
        request = request.bearer_auth(token);
    }
    run_configuration::check_deadline()?;
    let mut response = tokio::select! {_=stop.cancelled()=>bail!("device_run_canceled"),v=request.send()=>v.context("device_model_transport_failed_no_retry")?};
    ensure!(
        response.status().is_success(),
        "device_model_rejected_request_no_retry"
    );
    let mut raw = vec![];
    loop {
        let chunk = tokio::select! {_=stop.cancelled()=>bail!("device_run_canceled"),v=response.chunk()=>v?};
        let Some(chunk) = chunk else { break };
        ensure!(
            raw.len() + chunk.len() <= 4 * 1024 * 1024,
            "device_model_response_bound"
        );
        raw.extend_from_slice(&chunk);
    }
    let mut value: Value =
        serde_json::from_slice(&raw).context("device_model_original_response_invalid")?;
    ensure!(value.is_object(), "device_model_original_response_invalid");
    engine.admit(run,&format!("{key}/received"),json!({"phase":"model_received","scope":key,"request_sha256":sha,"response_sha256":digest(&raw),"response":value}))?;
    if model.api == "ollama" {
        let mut message = value["message"].clone();
        ensure!(message.is_object(), "device_ollama_message_missing");
        if let Some(calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) {
            for (i, call) in calls.iter_mut().enumerate() {
                call["id"] = json!(format!("ollama-{i}"));
                call["type"] = json!("function");
                let args = call["function"]["arguments"].clone();
                if !args.is_string() {
                    call["function"]["arguments"] = json!(serde_json::to_string(&args)?);
                }
            }
        }
        let prompt = value["prompt_eval_count"].as_u64();
        let completion = value["eval_count"].as_u64();
        value = json!({"choices":[{"message":message}],"usage":{"prompt_tokens":prompt,"completion_tokens":completion,"total_tokens":prompt.zip(completion).map(|(p,c)|p.saturating_add(c))},"original_provider_api":"ollama"});
    }
    let measured = value["usage"]["prompt_tokens"]
        .as_u64()
        .zip(value["usage"]["completion_tokens"].as_u64());
    if let Some((input, output)) = measured {
        ensure!(
            output <= u64::from(settings.budgets.max_output_tokens)
                && input
                    .checked_add(output)
                    .is_some_and(|used| used <= reserved_tokens),
            "device_model_reported_usage_exceeds_original_reservation"
        );
    }
    value["device_accounting"] =
        json!({"reserved_tokens":reserved_tokens,"usage_complete":measured.is_some()});
    Ok(value)
}
