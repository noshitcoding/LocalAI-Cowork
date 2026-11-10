//! Concrete legacy tool operations with local original preparation, consent and receive.
use super::*;
pub(super) const NAMES: &[&str] = &[
    "read_file",
    "edit_file",
    "create_directory",
    "copy_path",
    "move_path",
    "delete_file",
    "glob",
    "grep",
    "bash",
    "git",
    "web_fetch",
    "web_search",
];
pub(super) fn definitions() -> Vec<Value> {
    let string = json!({"type":"string"});
    vec![
        definition(
            "read_file",
            "Read one pinned UTF-8 file from this run only.",
            json!({"path":string}),
            &["path"],
        ),
        definition(
            "edit_file",
            "Replace exact original text in one pinned run file after approval.",
            json!({"path":string,"old_text":string,"new_text":string,"replace_all":{"type":"boolean"}}),
            &["path", "old_text", "new_text"],
        ),
        definition(
            "create_directory",
            "Create one new directory under the current run.",
            json!({"path":string}),
            &["path"],
        ),
        definition(
            "copy_path",
            "Copy one exact pinned file to a new path in this run.",
            json!({"source":string,"destination":string}),
            &["source", "destination"],
        ),
        definition(
            "move_path",
            "Move one exact pinned file to a new path in this run.",
            json!({"source":string,"destination":string}),
            &["source", "destination"],
        ),
        definition(
            "delete_file",
            "Delete exactly one pinned regular file in this run after approval.",
            json!({"path":string}),
            &["path"],
        ),
        definition(
            "glob",
            "Find run files by a bounded case-sensitive path pattern.",
            json!({"pattern":string}),
            &["pattern"],
        ),
        definition(
            "grep",
            "Search exact text in bounded UTF-8 run files, returning original line numbers.",
            json!({"query":string,"pattern":string}),
            &["query"],
        ),
        definition(
            "bash",
            "Execute one approved command with the operator-selected shell and native rights.",
            json!({"command":string,"cwd":string,"timeout_seconds":{"type":"integer","minimum":1,"maximum":600}}),
            &["command"],
        ),
        definition(
            "git",
            "Execute a concrete Git builtin in the pinned run tree; network operations require an exact HTTPS remote URL.",
            json!({"arguments":{"type":"array","items":string},"cwd":string,"remote_url":string,"destination":string,"timeout_seconds":{"type":"integer","minimum":1,"maximum":600}}),
            &["arguments"],
        ),
        definition(
            "web_fetch",
            "Fetch one approved HTTPS URL with pinned resolved addresses, no redirects or ambient credentials.",
            json!({"url":string}),
            &["url"],
        ),
        definition(
            "web_search",
            "Search through the operator-selected HTTPS query endpoint; returned content remains untrusted.",
            json!({"query":string}),
            &["query"],
        ),
    ]
}
#[expect(
    clippy::too_many_arguments,
    reason = "Interop tool execution retains explicit owner rights, run lease, original approval source, tool arguments and cancellation."
)]
pub(super) async fn call(
    engine: &ExecutionEngine,
    control: &dyn DeviceControl,
    run: Uuid,
    lease: Uuid,
    source: Uuid,
    name: &str,
    arguments: Value,
    stop: &CancellationToken,
) -> Result<Value> {
    ensure!(
        NAMES.contains(&name) && engine.config.native_owner_rights && arguments.is_object(),
        "device_interop_tool_not_allowed"
    );
    ensure!(
        serde_json::to_vec(&arguments)?.len() <= 1024 * 1024,
        "device_interop_arguments_bound"
    );
    for key in ["path", "source", "destination", "cwd"] {
        if let Some(path) = arguments[key].as_str().filter(|v| !v.is_empty()) {
            workspace::relative(path)?;
        }
    }
    let _workspace_lease = engine
        .toolbox
        .workspace_lease(&run.to_string())
        .map_err(|_| anyhow::anyhow!("device_workspace_busy"))?;
    let workspace = engine
        .toolbox
        .workspace(&run.to_string())
        .map_err(|_| anyhow::anyhow!("device_workspace_unavailable"))?;
    let directory = engine
        .config
        .data_directory
        .join("interop")
        .join(run.to_string())
        .join(source.to_string());
    private_directory(&directory)?;
    let hooks = directory.join("empty-hooks");
    private_directory(&hooks)?;
    let helper = receipts::helper(
        &engine.config.data_directory.join("interop-adapters"),
        "device-v1-tools.py",
        include_bytes!("device_v1_tools.py"),
    )?;
    let python = engine
        .config
        .programs
        .get("python")
        .context("device_interop_python_missing")?
        .clone();
    let timeout = arguments["timeout_seconds"].as_u64().unwrap_or(120);
    ensure!(
        (1..=600).contains(&timeout),
        "device_interop_timeout_invalid"
    );
    let mut request = json!({"phase":"prepare","tool":name,"arguments":arguments,"timeout":timeout,"allowed_hosts":engine.config.interop.allowed_hosts,"web_search_url":engine.config.interop.web_search_url,"empty_hooks":hooks});
    if matches!(name, "git" | "bash") {
        request["program"] = json!(
            engine
                .config
                .programs
                .get(if name == "git" { "git" } else { "shell" })
                .context("device_interop_program_missing")?
        );
    }
    receipts::write(&directory, "prepare.json", &request)?;
    let prepared = receipts::supervise(
        engine,
        run,
        python.clone(),
        vec![
            "-I".into(),
            helper.to_string_lossy().into(),
            workspace.to_string_lossy().into(),
            directory.join("prepare.json").to_string_lossy().into(),
        ],
        workspace.clone(),
        30,
        stop,
    )
    .await?;
    let action = json!({"format":"device-original-tool-plan/1","run_id":run,"lease_sha256":digest(lease.as_bytes()),"source_request_id":source,"tool":name,"prepared":prepared,"programs":engine.config.programs,"mode":"operator_native","automatic_replay":false});
    let sha = digest(&serde_json::to_vec(&action)?);
    receipts::write(&directory, "original-plan.json", &action)?;
    if !engine
        .approval(control, run, lease, source, action.clone(), stop)
        .await?
    {
        return Ok(json!({"state":"rejected","performed":false}));
    }
    engine.admit(
        run,
        &source.to_string(),
        json!({"phase":"interop_original_dispatched","request_sha256":sha,"action":action}),
    )?;
    engine
        .checkpoint(
            control,
            run,
            lease,
            source,
            json!({"tool":name,"request_sha256":sha}),
        )
        .await?;
    ensure!(!stop.is_cancelled(), "device_run_canceled");
    request["phase"] = json!("execute");
    request["arguments"] = prepared["arguments"].clone();
    request["pins"] = prepared["inputs"].clone();
    request["outputs"] = prepared["outputs"].clone();
    receipts::write(&directory, "execute.json", &request)?;
    let result = receipts::supervise(
        engine,
        run,
        python,
        vec![
            "-I".into(),
            helper.to_string_lossy().into(),
            workspace.to_string_lossy().into(),
            directory.join("execute.json").to_string_lossy().into(),
        ],
        workspace,
        timeout,
        stop,
    )
    .await?;
    let receipt = json!({"format":"device-original-tool-receipt/1","run_id":run,"source_request_id":source,"request_sha256":sha,"response":result,"response_sha256":digest(&serde_json::to_vec(&result)?),"physical_process_closed":true,"automatic_replay":false});
    receipts::write(&directory, "original-receipt.json", &receipt)?;
    engine.admit(run, &format!("{source}/settled"), receipt.clone())?;
    Ok(engine.redact_value(receipt))
}
