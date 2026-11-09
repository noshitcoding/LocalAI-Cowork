use super::*;
pub(super) fn definition() -> Value {
    super::definition(
        "browser_visible",
        "Use a newly owned visible Chromium window. Reuse the returned session_id; every concrete action requires current consent.",
        json!({"session_id":{"type":"string"},"operation":{"type":"string","enum":["navigate","dom","screenshot","click","fill"]},"url":{"type":"string"},"selector":{"type":"string"},"text":{"type":"string"}}),
        &["operation"],
    )
}
pub(super) async fn call(
    engine: &ExecutionEngine,
    control: &dyn DeviceControl,
    run: Uuid,
    lease: Uuid,
    source: Uuid,
    args: Value,
    stop: &CancellationToken,
) -> Result<Value> {
    ensure!(
        engine.config.native_owner_rights
            && engine
                .config
                .allowed_tools
                .iter()
                .any(|n| n == "browser_visible"),
        "device_visible_browser_not_allowed"
    );
    let session = args["session_id"]
        .as_str()
        .map(Uuid::parse_str)
        .transpose()?
        .unwrap_or_else(Uuid::new_v4);
    let id = format!("visible-{session}");
    let mut directory = if args["session_id"].is_null() {
        None
    } else {
        Some(engine.processes.directory(&id, run)?)
    };
    if cfg!(windows) {
        ensure!(
            crate::desktop::available(),
            "device_browser_requires_unlocked_interactive_session"
        );
    }
    let action = json!({"tool":"browser_visible","session_id":session,"arguments":args,"mode":"visible_operator_native","new_owned_context":directory.is_none(),"automatic_replay":false});
    if !engine
        .approval(control, run, lease, source, action.clone(), stop)
        .await?
    {
        return Ok(json!({"state":"rejected","performed":false}));
    }
    engine.admit(
        run,
        &source.to_string(),
        json!({"phase":"visible_browser_dispatched","action":action}),
    )?;
    engine
        .checkpoint(control, run, lease, source, action)
        .await?;
    ensure!(!stop.is_cancelled(), "device_run_canceled");
    if directory.is_none() {
        ensure!(
            args["session_id"].is_null(),
            "device_visible_browser_session_unowned_no_restore"
        );
        let d = engine
            .config
            .data_directory
            .join("visible-browser")
            .join(run.to_string())
            .join(session.to_string());
        private_directory(&d)?;
        let chromium = engine
            .config
            .programs
            .get("chromium")
            .context("device_chromium_missing")?;
        let helper = receipts::helper(
            &engine.config.data_directory.join("interop-adapters"),
            "visible-browser.py",
            include_bytes!("device_visible_browser.py"),
        )?;
        receipts::write(
            &d,
            "config.json",
            &json!({"session_id":session,"epoch":Uuid::new_v4(),"chromium":chromium,"chromium_sha256":digest(&fs::read(chromium)?)}),
        )?;
        let mut environment = engine.config.environment.clone();
        for name in [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_RUNTIME_DIR",
            "XDG_SESSION_ID",
            "DBUS_SESSION_BUS_ADDRESS",
        ] {
            if let Ok(value) = std::env::var(name) {
                environment.insert(name.into(), value);
            }
        }
        engine.processes.launch(
            &id,
            run,
            d.clone(),
            cowork_executor::TrustedLaunch {
                program: engine
                    .config
                    .programs
                    .get("python")
                    .context("device_python_missing")?
                    .clone(),
                arguments: vec![
                    "-I".into(),
                    helper.to_string_lossy().into(),
                    d.to_string_lossy().into(),
                ],
                directory: engine
                    .toolbox
                    .workspace(&run.to_string())
                    .map_err(|_| anyhow::anyhow!("device_workspace_unavailable"))?,
                environment,
                timeout: Duration::from_secs(3600),
                capture_bytes_per_stream: 4096,
            },
        )?;
        directory = Some(d);
    }
    let d = directory.context("device_visible_browser_missing")?;
    let ready = process::wait_json(&d.join("ready.json"), stop, 60).await?;
    ensure!(
        ready["ok"] == true && ready["session_id"] == session.to_string(),
        "device_visible_browser_original_context_differs"
    );
    let request = json!({"source":source,"epoch":ready["epoch"],"timeout":120,"arguments":args});
    let raw = serde_json::to_vec(&request)?;
    receipts::write(&d, &format!("{source}.request.json"), &request)?;
    let response =
        process::wait_json(&d.join(format!("{source}.response.json")), stop, 120).await?;
    ensure!(
        response["ok"] == true
            && response["source"] == source.to_string()
            && response["epoch"] == ready["epoch"]
            && response["request_sha256"] == digest(&raw),
        "device_visible_browser_original_result_unknown"
    );
    let mut published = Value::Null;
    if let Some(shot) = response.get("screenshot").filter(|v| !v.is_null()) {
        let expected = format!("{source}.png");
        ensure!(
            shot["file"] == expected,
            "device_browser_foreign_screenshot"
        );
        let workspace_lease = engine
            .toolbox
            .workspace_lease(&run.to_string())
            .map_err(|_| anyhow::anyhow!("device_workspace_busy"))?;
        published = office::publish(
            engine,
            run,
            &d,
            &expected,
            &format!("artifacts/browser-{source}.png"),
            shot,
        )?;
        drop(workspace_lease);
    }
    let result = json!({"format":"device-visible-browser-receipt/1","session_id":session,"source_request_id":source,"request_sha256":digest(&raw),"response":response,"published":published,"automatic_replay":false});
    engine.admit(run, &format!("{source}/settled"), result.clone())?;
    Ok(engine.redact_value(result))
}
